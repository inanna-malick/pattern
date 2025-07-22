//! Database-backed agent implementation

use async_trait::async_trait;
use compact_str::CompactString;
use futures::StreamExt;
use std::sync::Arc;
use surrealdb::Surreal;
use tokio::sync::RwLock;

// use crate::tool::builtin::BuiltinTools;

use crate::{
    CoreError, MemoryBlock, ModelProvider, Result, UserId,
    agent::{Agent, AgentMemoryRelation, AgentState, AgentType, MemoryAccessLevel},
    context::{AgentContext, ContextConfig},
    db::{DatabaseError, ops, schema},
    embeddings::EmbeddingProvider,
    id::AgentId,
    memory::Memory,
    message::{Message, Request, Response},
    model::ResponseOptions,
    tool::{DynamicTool, ToolRegistry},
};

/// A concrete agent implementation backed by the database
pub struct DatabaseAgent<C, M, E>
where
    C: surrealdb::Connection,
{
    /// The user who owns this agent
    pub user_id: UserId,
    /// The agent's context (includes all state)
    context: Arc<RwLock<AgentContext>>,

    /// model provider
    model: Arc<RwLock<M>>,

    /// model configuration
    pub chat_options: Arc<RwLock<Option<ResponseOptions>>>,
    /// Database connection
    db: Arc<Surreal<C>>,

    /// Embedding provider for semantic search
    embeddings: Option<Arc<E>>,
}

impl<C, M, E> DatabaseAgent<C, M, E>
where
    C: surrealdb::Connection,
    M: ModelProvider + 'static,
    E: EmbeddingProvider + 'static,
{
    /// Create a new database-backed agent
    /// Create a new database agent
    ///
    /// This creates a new agent instance with the provided configuration.
    /// Memory blocks and message history should be loaded separately if needed.
    pub fn new(
        agent_id: AgentId,
        user_id: UserId,
        agent_type: AgentType,
        name: String,
        system_prompt: String,
        memory: Memory,
        db: Arc<Surreal<C>>,
        model: Arc<RwLock<M>>,
        tools: ToolRegistry,
        embeddings: Option<Arc<E>>,
    ) -> Self {
        let context_config = if !system_prompt.is_empty() {
            ContextConfig {
                base_instructions: system_prompt,
                ..Default::default()
            }
        } else {
            ContextConfig::default()
        };
        // Build AgentContext with tools
        let mut context =
            AgentContext::new(agent_id, name, agent_type, memory, tools, context_config);
        context.handle.state = AgentState::Ready;

        // Register built-in tools
        // TODO: Fix tool schema for Gemini compatibility
        // let builtin = BuiltinTools::default_for_agent(context.handle());
        // builtin.register_all(&context.tools);

        Self {
            user_id,
            context: Arc::new(RwLock::new(context)),
            chat_options: Arc::new(RwLock::const_new(None)),
            db,
            embeddings,
            model,
        }
    }

    /// Create a DatabaseAgent from a persisted AgentRecord
    ///
    /// This reconstructs the runtime agent from its stored state.
    /// The model provider, tools, and embeddings are injected at runtime.
    pub async fn from_record(
        record: crate::agent::AgentRecord,
        db: Arc<Surreal<C>>,
        model: Arc<RwLock<M>>,
        tools: ToolRegistry,
        embeddings: Option<Arc<E>>,
    ) -> Result<Self> {
        tracing::debug!(
            "Creating DatabaseAgent from record: agent_id={}, name={}",
            record.id,
            record.name
        );
        tracing::debug!(
            "Record has {} messages and {} memory blocks",
            record.messages.len(),
            record.memories.len()
        );

        // Create memory with the owner
        let memory = Memory::with_owner(record.owner_id);

        // Load memory blocks from the record's relations
        for (memory_block, _relation) in &record.memories {
            tracing::debug!(
                "Loading memory block: label={}, size={} chars",
                memory_block.label,
                memory_block.value.len()
            );
            memory.create_block(memory_block.label.clone(), memory_block.value.clone())?;
            if let Some(mut block) = memory.get_block_mut(&memory_block.label) {
                block.id = memory_block.id;
                block.description = memory_block.description.clone();
                block.metadata = memory_block.metadata.clone();
                block.embedding_model = memory_block.embedding_model.clone();
                block.created_at = memory_block.created_at;
                block.updated_at = memory_block.updated_at;
                block.is_active = memory_block.is_active;
            }
        }

        // Build the context config from the record
        let context_config = record.to_context_config();

        // Create the agent with the loaded configuration
        let agent = Self::new(
            record.id,
            record.owner_id,
            record.agent_type.clone(),
            record.name.clone(),
            context_config.base_instructions.clone(),
            memory,
            db,
            model,
            tools,
            embeddings,
        );

        // Restore the agent state
        {
            let mut context = agent.context.write().await;
            context.handle.state = record.state;
        }

        // Restore message history and compression state
        {
            let context = agent.context.read().await;
            let mut history = context.history.write().await;
            history.compression_strategy = record.compression_strategy.clone();
            history.message_summary = record.message_summary.clone();

            // Load active messages from relations (already ordered by position)
            let mut loaded_messages = 0;
            for (message, relation) in &record.messages {
                if relation.message_type == crate::message::MessageRelationType::Active {
                    let content_preview = match &message.content {
                        crate::message::MessageContent::Text(text) => {
                            text.chars().take(50).collect::<String>()
                        }
                        crate::message::MessageContent::ToolCalls(tool_calls) => {
                            format!("ToolCalls: {} calls", tool_calls.len())
                        }
                        crate::message::MessageContent::ToolResponses(tool_responses) => {
                            format!("ToolResponses: {} responses", tool_responses.len())
                        }
                        crate::message::MessageContent::Parts(parts) => {
                            format!("Parts: {} parts", parts.len())
                        }
                    };
                    tracing::trace!(
                        "Loading message: id={:?}, role={:?}, content_preview={:?}",
                        message.id,
                        message.role,
                        content_preview
                    );
                    history.messages.push(message.clone());
                    loaded_messages += 1;
                }
            }
            tracing::debug!("Loaded {} active messages into history", loaded_messages);
        }

        // Restore statistics
        {
            let context = agent.context.write().await;
            let mut metadata = context.metadata.write().await;
            metadata.created_at = record.created_at;
            metadata.last_active = record.last_active;
            metadata.total_messages = record.total_messages;
            metadata.total_tool_calls = record.total_tool_calls;
            metadata.context_rebuilds = record.context_rebuilds;
            metadata.compression_events = record.compression_events;
        }

        Ok(agent)
    }

    /// Store the current agent state to the database
    pub async fn store(&self) -> Result<()> {
        // Create an AgentRecord from the current state
        let (agent_id, name, agent_type, state) = {
            let context = self.context.read().await;
            (
                context.handle.agent_id,
                context.handle.name.clone(),
                context.handle.agent_type.clone(),
                context.handle.state,
            )
        };

        let (base_instructions, max_messages) = {
            let context = self.context.read().await;
            (
                context.context_config.base_instructions.clone(),
                context.context_config.max_context_messages,
            )
        };

        let (total_messages, total_tool_calls, context_rebuilds, compression_events, last_active) = {
            let context = self.context.read().await;
            let metadata = context.metadata.read().await;
            (
                metadata.total_messages,
                metadata.total_tool_calls,
                metadata.context_rebuilds,
                metadata.compression_events,
                metadata.last_active,
            )
        };

        let now = chrono::Utc::now();
        let agent_record = crate::agent::AgentRecord {
            id: agent_id,
            name,
            agent_type,
            state,
            base_instructions,
            max_messages,
            owner_id: self.user_id,
            total_messages,
            total_tool_calls,
            context_rebuilds,
            compression_events,
            created_at: now, // This will be overwritten if agent already exists
            updated_at: now,
            last_active,
            ..Default::default()
        };

        // Store the agent record
        agent_record.store_with_relations(&self.db).await?;
        Ok(())
    }

    /// Start background task to sync memory updates via live queries
    pub async fn start_memory_sync(self: Arc<Self>) -> Result<()> {
        let (agent_id, memory) = {
            let context = self.context.read().await;
            (context.handle.agent_id, context.handle.memory.clone())
        };
        let db = Arc::clone(&self.db);

        // Spawn background task to handle updates
        tokio::spawn(async move {
            use futures::StreamExt;
            tracing::info!("Memory sync task started for agent {}", agent_id);

            // Subscribe to all memory changes for this agent
            let stream = match ops::subscribe_to_agent_memory_updates(&db, agent_id).await {
                Ok(s) => {
                    tracing::info!(
                        "Successfully subscribed to memory updates for agent {} - live query active",
                        agent_id
                    );
                    s
                }
                Err(e) => {
                    tracing::error!("Failed to subscribe to memory updates: {:?}", e);
                    return;
                }
            };

            futures::pin_mut!(stream);

            while let Some((action, memory_block)) = stream.next().await {
                tracing::info!(
                    "🔔 Agent {} received live query notification: {:?} for block '{}' with content: '{}'",
                    agent_id,
                    action,
                    memory_block.label,
                    memory_block.value
                );

                match action {
                    surrealdb::Action::Create => {
                        tracing::info!(
                            "Agent {} creating memory block '{}'",
                            agent_id,
                            memory_block.label
                        );
                        // Create new block
                        if let Err(e) =
                            memory.create_block(memory_block.label.clone(), &memory_block.value)
                        {
                            tracing::error!(
                                "Failed to create memory block {}: {}",
                                memory_block.label,
                                e
                            );
                        }
                        // Update other fields if present
                        if let Some(mut block) = memory.get_block_mut(&memory_block.label) {
                            block.id = memory_block.id;
                            // owner_id is now managed via RELATE, not stored on the entity
                            block.description = memory_block.description;
                            block.metadata = memory_block.metadata;
                            block.embedding_model = memory_block.embedding_model;
                            block.created_at = memory_block.created_at;
                            block.updated_at = memory_block.updated_at;
                            block.is_active = memory_block.is_active;
                        }
                    }

                    surrealdb::Action::Update => {
                        // Update or create the block
                        if memory.get_block(&memory_block.label).is_some() {
                            tracing::info!(
                                "✅ Agent {} updating existing memory '{}' with content: '{}'",
                                agent_id,
                                memory_block.label,
                                memory_block.value
                            );
                            // Update existing block
                            if let Err(e) =
                                memory.update_block_value(&memory_block.label, &memory_block.value)
                            {
                                tracing::error!(
                                    "Failed to update memory block {}: {}",
                                    memory_block.label,
                                    e
                                );
                            }
                        } else {
                            tracing::info!(
                                "Agent {} creating new memory block '{}' (from update)",
                                agent_id,
                                memory_block.label
                            );
                            // Create new block
                            if let Err(e) =
                                memory.create_block(memory_block.label.clone(), &memory_block.value)
                            {
                                tracing::error!(
                                    "Failed to create memory block {}: {}",
                                    memory_block.label,
                                    e
                                );
                            }
                        }

                        // Update other fields if present
                        if let Some(mut block) = memory.get_block_mut(&memory_block.label) {
                            block.id = memory_block.id;
                            // owner_id is now managed via RELATE, not stored on the entity
                            block.description = memory_block.description;
                            block.metadata = memory_block.metadata;
                            block.embedding_model = memory_block.embedding_model;
                            block.created_at = memory_block.created_at;
                            block.updated_at = memory_block.updated_at;
                            block.is_active = memory_block.is_active;
                        }
                    }
                    surrealdb::Action::Delete => {
                        tracing::info!(
                            "🗑️ Agent {} received DELETE for memory block '{}'",
                            agent_id,
                            memory_block.label
                        );
                        memory.remove_block(&memory_block.label);
                    }
                    _ => {
                        tracing::debug!("Ignoring action {:?} for memory block", action);
                    }
                }
            }

            tracing::warn!(
                "⚠️ Memory sync task exiting for agent {} - stream ended",
                agent_id
            );
        });

        Ok(())
    }

    /// Start background task to sync agent stats updates with UI
    pub async fn start_stats_sync(&self) -> Result<()> {
        let db = Arc::clone(&self.db);
        let (agent_id, metadata, history) = {
            let context = self.context.read().await;
            (
                context.handle.agent_id,
                Arc::clone(&context.metadata),
                Arc::clone(&context.history),
            )
        };

        tokio::spawn(async move {
            let stream = match crate::db::ops::subscribe_to_agent_stats(&db, agent_id).await {
                Ok(stream) => stream,
                Err(e) => {
                    tracing::error!("Failed to subscribe to agent stats updates: {}", e);
                    return;
                }
            };

            tracing::info!("📊 Agent {} stats sync task started", agent_id);

            tokio::pin!(stream);
            while let Some((action, agent_record)) = stream.next().await {
                match action {
                    surrealdb::Action::Update => {
                        tracing::debug!(
                            "📊 Agent {} received stats update - messages: {}, tool calls: {}",
                            agent_id,
                            agent_record.total_messages,
                            agent_record.total_tool_calls
                        );

                        // Update metadata with the latest stats
                        let mut meta = metadata.write().await;
                        meta.total_messages = agent_record.total_messages;
                        meta.total_tool_calls = agent_record.total_tool_calls;
                        meta.last_active = agent_record.last_active;

                        // Update compression events if the message summary changed
                        let history_read = history.read().await;
                        if agent_record.message_summary != history_read.message_summary {
                            meta.compression_events += 1;
                        }
                        drop(history_read);

                        // Update history if message summary changed
                        if agent_record.message_summary.is_some() {
                            let mut history_write = history.write().await;
                            history_write.message_summary = agent_record.message_summary;
                        }
                    }
                    _ => {
                        tracing::debug!("Ignoring action {:?} for agent stats", action);
                    }
                }
            }

            tracing::warn!(
                "⚠️ Stats sync task exiting for agent {} - stream ended",
                agent_id
            );
        });

        Ok(())
    }

    /// Compress messages and persist archival to database
    pub async fn compress_messages_if_needed(&self) -> Result<()> {
        // Check if compression is needed
        let needs_compression = {
            let context = self.context.read().await;
            let history = context.history.read().await;
            history.messages.len() > context.context_config.max_context_messages
        };

        if !needs_compression {
            return Ok(());
        }

        // Get compression strategy and create compressor
        let compression_strategy = {
            let context = self.context.read().await;
            let history = context.history.read().await;
            history.compression_strategy.clone()
        };

        // Create a wrapper for the model provider that can be passed to the compressor
        #[derive(Debug)]
        struct ModelProviderWrapper<M: ModelProvider> {
            model: Arc<RwLock<M>>,
        }

        #[async_trait]
        impl<M: ModelProvider> ModelProvider for ModelProviderWrapper<M> {
            fn name(&self) -> &str {
                "compression_wrapper"
            }

            async fn complete(
                &self,
                options: &crate::model::ResponseOptions,
                request: crate::message::Request,
            ) -> crate::Result<crate::message::Response> {
                let model = self.model.read().await;
                model.complete(options, request).await
            }

            async fn list_models(&self) -> crate::Result<Vec<crate::model::ModelInfo>> {
                let model = self.model.read().await;
                model.list_models().await
            }

            async fn supports_capability(
                &self,
                model: &str,
                capability: crate::model::ModelCapability,
            ) -> bool {
                let provider = self.model.read().await;
                provider.supports_capability(model, capability).await
            }

            async fn count_tokens(&self, model: &str, content: &str) -> crate::Result<usize> {
                let provider = self.model.read().await;
                provider.count_tokens(model, content).await
            }
        }

        let model_wrapper = ModelProviderWrapper {
            model: self.model.clone(),
        };

        let compressor = crate::context::MessageCompressor::new(compression_strategy)
            .with_model_provider(Box::new(model_wrapper));

        // Get messages to compress
        let messages_to_compress = {
            let context = self.context.read().await;
            let history = context.history.read().await;
            history.messages.clone()
        };

        // Perform compression
        let max_context_messages = {
            let context = self.context.read().await;
            context.context_config.max_context_messages
        };
        let compression_result = compressor
            .compress(messages_to_compress, max_context_messages)
            .await?;

        // Collect archived message IDs
        let archived_ids: Vec<crate::MessageId> = compression_result
            .archived_messages
            .iter()
            .map(|msg| msg.id.clone())
            .collect();

        // Apply compression to state
        {
            let context = self.context.read().await;
            let mut history = context.history.write().await;

            // Move compressed messages to archive
            history
                .archived_messages
                .extend(compression_result.archived_messages);

            // Update active messages
            history.messages = compression_result.active_messages;

            // Update or append to summary
            if let Some(new_summary) = compression_result.summary {
                if let Some(existing_summary) = &mut history.message_summary {
                    *existing_summary = format!("{}\n\n{}", existing_summary, new_summary);
                } else {
                    history.message_summary = Some(new_summary.clone());
                }

                // Also update the agent record's message summary in background
                let db = Arc::clone(&self.db);
                let agent_id = context.handle.agent_id;
                let summary = new_summary;
                tokio::spawn(async move {
                    let query = r#"
                        UPDATE agent SET
                            message_summary = $summary,
                            updated_at = time::now()
                        WHERE id = $id
                    "#;

                    if let Err(e) = db
                        .query(query)
                        .bind(("id", surrealdb::RecordId::from(agent_id)))
                        .bind(("summary", summary))
                        .await
                    {
                        tracing::error!("Failed to update agent message summary: {:?}", e);
                    }
                });
            }

            history.last_compression = chrono::Utc::now();
        }

        // Update metadata
        {
            let context = self.context.read().await;
            let mut metadata = context.metadata.write().await;
            metadata.compression_events += 1;
        }

        // Persist archived message IDs to database in background
        if !archived_ids.is_empty() {
            let db = Arc::clone(&self.db);
            let agent_id = {
                let context = self.context.read().await;
                context.handle.agent_id
            };
            tokio::spawn(async move {
                if let Err(e) =
                    crate::db::ops::archive_agent_messages(&db, agent_id, &archived_ids).await
                {
                    tracing::error!("Failed to archive messages in database: {:?}", e);
                } else {
                    tracing::info!(
                        "Successfully archived {} messages for agent {} in database",
                        archived_ids.len(),
                        agent_id
                    );
                }
            });
        }

        Ok(())
    }
}

impl<C, M, E> std::fmt::Debug for DatabaseAgent<C, M, E>
where
    C: surrealdb::Connection,
    M: ModelProvider,
    E: EmbeddingProvider,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Can't await in Debug, so we'll use try_read
        if let Ok(context) = self.context.try_read() {
            f.debug_struct("DatabaseAgent")
                .field("agent_id", &context.handle.agent_id)
                .field("name", &context.handle.name)
                .field("agent_type", &context.handle.agent_type)
                .field("user_id", &self.user_id)
                .field("state", &context.handle.state)
                .field("has_embeddings", &self.embeddings.is_some())
                .field("memory_blocks", &context.handle.memory.list_blocks().len())
                .finish()
        } else {
            f.debug_struct("DatabaseAgent")
                .field("user_id", &self.user_id)
                .field("has_embeddings", &self.embeddings.is_some())
                .field("<locked>", &"...")
                .finish()
        }
    }
}

#[async_trait]
impl<C, M, E> Agent for DatabaseAgent<C, M, E>
where
    C: surrealdb::Connection + std::fmt::Debug + 'static,
    M: ModelProvider + 'static,
    E: EmbeddingProvider + 'static,
{
    fn id(&self) -> AgentId {
        // These methods are synchronous, so we need to store these values outside the lock
        // For now, we'll panic if we can't get the lock - this should be rare
        futures::executor::block_on(async { self.context.read().await.handle.agent_id })
    }

    fn name(&self) -> String {
        futures::executor::block_on(async { self.context.read().await.handle.name.clone() })
    }

    fn agent_type(&self) -> AgentType {
        futures::executor::block_on(async { self.context.read().await.handle.agent_type.clone() })
    }

    async fn process_message(&self, message: Message) -> Result<Response> {
        let agent_id = {
            let mut context = self.context.write().await;
            context.handle.state = AgentState::Processing;

            // Clone what we need for the background task
            let db = Arc::clone(&self.db);
            let agent_id = context.handle.agent_id;
            let message_clone = message.clone();

            // Fire off message persistence in background
            let _handle1 = tokio::spawn(async move {
                if let Err(e) = crate::db::ops::persist_agent_message(
                    &db,
                    agent_id,
                    &message_clone,
                    crate::message::MessageRelationType::Active,
                )
                .await
                {
                    tracing::error!("Failed to persist incoming message: {:?}", e);
                }
            });

            // Add message to context immediately

            context.add_message(message).await;
            agent_id
        };

        // Build the memory context for the LLM
        let memory_context = {
            let context = self.context.read().await;
            context.build_context().await?
        };

        tracing::debug!(
            "Sending request to LLM with {} messages (including the new one)",
            memory_context.messages.len()
        );
        for (i, msg) in memory_context.messages.iter().enumerate() {
            let content_preview = match &msg.content {
                crate::message::MessageContent::Text(text) => {
                    text.chars().take(50).collect::<String>()
                }
                _ => format!("{:?}", msg.content)
                    .chars()
                    .take(50)
                    .collect::<String>(),
            };
            tracing::debug!(
                "  Message[{}]: role={:?}, content_preview={:?}",
                i,
                msg.role,
                content_preview
            );
        }

        let request = Request {
            system: Some(vec![memory_context.system_prompt.clone()]),
            messages: memory_context.messages,
            tools: Some(memory_context.tools),
        };

        tracing::debug!(
            "Full request to LLM: system_prompt_len={}, messages={}, tools={}",
            request.system.as_ref().map(|s| s[0].len()).unwrap_or(0),
            request.messages.len(),
            request.tools.as_ref().map(|t| t.len()).unwrap_or(0)
        );

        let model = self.model.read().await;

        let options = self.chat_options.read().await;

        // Note: this will usually make a network request and will need to wait for generated tokens from the LLM. it's the slowest bit.
        let response = match model
            .complete(
                &options.clone().expect("should have options or default"),
                request,
            )
            .await
        {
            Ok(response) => {
                tracing::debug!(
                    "Got response from LLM: {} content parts, {} tool calls",
                    response.content.len(),
                    response.tool_calls.len()
                );
                response
            }
            Err(e) => {
                tracing::error!("Failed to get response from model: {:?}", e);

                // Check if it's a specific Gemini error about missing candidates
                let error_str = e.to_string();
                if error_str.contains("candidates/0/content/parts") {
                    tracing::warn!(
                        "Gemini returned an empty or blocked response. This might be due to safety filters or rate limits. Returning a fallback response."
                    );

                    // Create a fallback response when Gemini blocks content
                    Response {
                        content: vec![crate::message::MessageContent::Text(
                            "I apologize, but I'm unable to process that request at the moment. This might be due to content filters or rate limits. Please try rephrasing your message or try again later.".to_string()
                        )],
                        reasoning: None,
                        tool_calls: vec![],
                        metadata: crate::message::ResponseMetadata::default(),
                    }
                } else {
                    // For other errors, return them
                    return Err(e);
                }
            }
        };

        // Convert Response to Message
        let response_message = {
            let context = self.context.read().await;
            Message::from_response(&response, context.handle.agent_id)
        };

        // Clone for background persistence
        let db = Arc::clone(&self.db);
        let response_msg_clone = response_message.clone();

        // Fire off response persistence in background
        let _handle2 = tokio::spawn(async move {
            if let Err(e) = crate::db::ops::persist_agent_message(
                &db,
                agent_id,
                &response_msg_clone,
                crate::message::MessageRelationType::Active,
            )
            .await
            {
                tracing::error!("Failed to persist response message: {:?}", e);
            }
        });

        // Add response message to context immediately
        {
            let context = self.context.write().await;
            context.add_message(response_message).await;
        }

        // Process response and update state
        let (stats, db) = {
            let context = self.context.read().await;
            context.process_response(&response)?;

            // Update stats in background
            (context.get_stats().await, Arc::clone(&self.db))
        };
        let _handle3 = tokio::spawn(async move {
            if let Err(e) = crate::db::ops::update_agent_stats(&db, agent_id, &stats).await {
                tracing::error!("Failed to update agent stats: {:?}", e);
            }
        });

        // Reset state to Ready
        {
            let mut context = self.context.write().await;
            context.handle.state = AgentState::Ready;
        }

        Ok(response)
    }

    async fn get_memory(&self, key: &str) -> Result<Option<MemoryBlock>> {
        let context = self.context.read().await;
        Ok(context.handle.memory.get_block(key).map(|b| b.clone()))
    }

    async fn update_memory(&self, key: &str, memory: MemoryBlock) -> Result<()> {
        let agent_id = {
            let context = self.context.read().await;
            context.handle.agent_id
        };
        tracing::info!("🔧 Agent {} updating memory key '{}'", agent_id, key);

        // Update in context immediately
        {
            let context = self.context.write().await;
            context
                .update_memory_block(key, memory.value.clone())
                .await?
        };

        // Persist memory block in background
        let db = Arc::clone(&self.db);
        let agent_id = {
            let context = self.context.read().await;
            context.handle.agent_id
        };
        let memory_clone = memory.clone();

        let _handle = tokio::spawn(async move {
            if let Err(e) = crate::db::ops::persist_agent_memory(
                &db,
                agent_id,
                &memory_clone,
                MemoryAccessLevel::Write, // Agent has write access to its own memory
            )
            .await
            {
                tracing::error!("Failed to persist memory block: {:?}", e);
            }
        });

        Ok(())
    }

    async fn execute_tool(
        &self,
        tool_name: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        // Get the tool from the registry
        let (tool, agent_id) = {
            let context = self.context.read().await;
            let tool = context.tools.get(tool_name).ok_or_else(|| {
                CoreError::tool_not_found(
                    tool_name,
                    context
                        .tools
                        .list_tools()
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                )
            })?;
            (tool.clone(), context.handle.agent_id)
        };

        // Record start time
        let start_time = std::time::Instant::now();
        let created_at = chrono::Utc::now();

        // Execute the tool
        let result = tool.execute(params.clone()).await;

        // Calculate duration
        let duration_ms = start_time.elapsed().as_millis() as i64;

        // Update tool call count in metadata
        {
            let context = self.context.read().await;
            let mut metadata = context.metadata.write().await;
            metadata.total_tool_calls += 1;
        }

        let tool_call = schema::ToolCall {
            id: crate::id::ToolCallId::generate(),
            agent_id,
            tool_name: tool_name.to_string(),
            parameters: params,
            result: result.as_ref().unwrap_or(&serde_json::json!(null)).clone(),
            error: result.as_ref().err().map(|e| e.to_string()),
            duration_ms,
            created_at,
        };

        // Persist tool call in background
        let db = Arc::clone(&self.db);
        let _handle = tokio::spawn(async move {
            if let Err(e) = crate::db::ops::create_entity(&db, &tool_call).await {
                tracing::error!("Failed to persist tool call: {:?}", e);
            }
        });

        // Return the result
        result
    }

    async fn list_memory_keys(&self) -> Result<Vec<CompactString>> {
        let context = self.context.read().await;
        Ok(context.handle.memory.list_blocks())
    }

    async fn search_memory(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(CompactString, MemoryBlock, f32)>> {
        // For now, do a simple text search in memory blocks
        // TODO: Implement proper semantic search with embeddings
        let query_lower = query.to_lowercase();

        let mut results: Vec<(CompactString, MemoryBlock, f32)> = Vec::new();

        let blocks = {
            let context = self.context.read().await;
            context.handle.memory.get_all_blocks()
        };
        for block in blocks {
            let content_lower = block.value.to_lowercase();
            if content_lower.contains(&query_lower) {
                // Simple scoring based on match count
                let score =
                    content_lower.matches(&query_lower).count() as f32 / block.value.len() as f32;
                results.push((block.label.clone(), block, score));
            }
        }

        // Sort by score descending
        results.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);

        Ok(results)
    }

    async fn share_memory_with(
        &self,
        memory_key: &str,
        target_agent_id: AgentId,
        access_level: MemoryAccessLevel,
    ) -> Result<()> {
        // First verify the memory block exists
        let memory_block = {
            let context = self.context.read().await;
            context
                .handle
                .memory
                .get_block(memory_key)
                .ok_or_else(|| {
                    CoreError::memory_not_found(
                        &context.handle.agent_id,
                        memory_key,
                        context.handle.memory.list_blocks(),
                    )
                })?
                .clone()
        };

        // Create the memory relation for the target agent
        let db = Arc::clone(&self.db);
        let memory_id = memory_block.id.clone();
        let relation = AgentMemoryRelation {
            id: None,
            in_id: target_agent_id.clone(),
            out_id: memory_id.clone(),
            access_level,
            created_at: chrono::Utc::now(),
        };

        // Persist the sharing relationship in background
        let target_id_clone = target_agent_id.clone();
        let _handle = tokio::spawn(async move {
            if let Err(e) = crate::db::ops::create_relation_typed(&db, &relation).await {
                tracing::error!("Failed to share memory block: {:?}", e);
            } else {
                tracing::info!(
                    "Shared memory block {} with agent {} (access: {:?})",
                    memory_id,
                    target_id_clone,
                    access_level
                );
            }
        });

        Ok(())
    }

    async fn get_shared_memories(&self) -> Result<Vec<(AgentId, CompactString, MemoryBlock)>> {
        let db = Arc::clone(&self.db);
        let agent_id = {
            let context = self.context.read().await;
            context.handle.agent_id
        };

        // Query for all memory blocks shared with this agent
        let query = r#"
            SELECT
                out AS memory_block,
                in AS source_agent_id,
                access_level
            FROM agent_memories
            WHERE in = $agent_id
            FETCH out, in
        "#;

        let mut response = db
            .query(query)
            .bind(("agent_id", surrealdb::RecordId::from(&agent_id)))
            .await
            .map_err(|e| DatabaseError::QueryFailed(e))?;

        // Extract the shared memory relationships
        let results: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|e| DatabaseError::QueryFailed(e))?;

        let mut shared_memories = Vec::new();

        for result in results {
            if let (Some(memory_value), Some(source_id)) =
                (result.get("memory_block"), result.get("source_agent_id"))
            {
                // Parse the memory block
                let memory_block: MemoryBlock = serde_json::from_value(memory_value.clone())
                    .map_err(|e| DatabaseError::SerdeProblem(e))?;

                // Parse the source agent ID
                let source_agent_id: AgentId = serde_json::from_value(source_id.clone())
                    .map_err(|e| DatabaseError::SerdeProblem(e))?;

                shared_memories.push((source_agent_id, memory_block.label.clone(), memory_block));
            }
        }

        Ok(shared_memories)
    }

    async fn system_prompt(&self) -> Vec<String> {
        let context = self.context.read().await;
        match context.build_context().await {
            Ok(memory_context) => {
                // Split the built system prompt into logical sections
                memory_context
                    .system_prompt
                    .split("\n\n")
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            }
            Err(_) => {
                // If context building fails, return base instructions
                vec![context.context_config.base_instructions.clone()]
            }
        }
    }

    fn available_tools(&self) -> Vec<Box<dyn DynamicTool>> {
        // This is problematic because it's a sync function but we need async to read the lock
        // For now, we'll use block_on
        futures::executor::block_on(async {
            let context = self.context.read().await;
            context.tools.get_all_as_dynamic()
        })
    }

    fn state(&self) -> AgentState {
        // This is problematic because it's a sync function but we need async to read the lock
        // For now, we'll use block_on
        futures::executor::block_on(async {
            let context = self.context.read().await;
            context.handle.state
        })
    }

    /// Set the agent's state
    async fn set_state(&self, state: AgentState) -> Result<()> {
        // Update the state in the context
        {
            let mut context = self.context.write().await;
            context.handle.state = state;
            let mut metadata = context.metadata.write().await;
            metadata.last_active = chrono::Utc::now();
        }

        // Persist the state update in background
        let db = Arc::clone(&self.db);
        let agent_id = {
            let context = self.context.read().await;
            context.handle.agent_id
        };
        let _handle = tokio::spawn(async move {
            let query =
                "UPDATE agent SET state = $state, last_active = $last_active WHERE id = $id";
            if let Err(e) = db
                .query(query)
                .bind(("id", surrealdb::RecordId::from(&agent_id)))
                .bind(("state", serde_json::to_value(&state).unwrap()))
                .bind(("last_active", surrealdb::Datetime::from(chrono::Utc::now())))
                .await
            {
                tracing::error!("Failed to persist agent state update: {:?}", e);
            }
        });

        Ok(())
    }
}

// Extension trait for database operations
#[async_trait]
pub trait AgentDbExt<C: surrealdb::Connection> {
    async fn create_tool_call(&self, tool_call: schema::ToolCall) -> Result<()>;
}

#[async_trait]
impl<T, C> AgentDbExt<C> for T
where
    T: AsRef<surrealdb::Surreal<C>> + Sync,
    C: surrealdb::Connection,
{
    async fn create_tool_call(&self, tool_call: schema::ToolCall) -> Result<()> {
        // Store the tool call in the database
        crate::db::ops::create_entity(self.as_ref(), &tool_call).await?;
        Ok(())
    }
}
