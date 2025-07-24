# CLAUDE.md - Pattern ADHD Cognitive Support System

Pattern is a multi-agent ADHD support system inspired by MemGPT's architecture to provide external executive function through specialized cognitive agents.

## Project Status

**Current Status**: Core foundation complete, ready for feature development

### 🚧 Current Development Priorities
1. **Model Configuration** - ✅ COMPLETE (2025-07-24)
   - Created `pattern_core/src/model/defaults.rs` with comprehensive model registry
   - Implemented `enhance_model_info()` to fix provider-supplied ModelInfo
   - Added accurate July 2025 model specifications for all major providers
   - Dynamic `calculate_max_tokens()` respects model-specific limits
   - Smart caching with CacheControl::Ephemeral for Anthropic optimization
   - Integrated MessageCompressor with multiple compression strategies

2. **Agent Groups** - ✅ COMPLETE (needs user testing) - Fully usable via CLI
3. **Task Management System** - ADHD-aware task breakdown and tracking
4. **MCP Tools Integration** - Task-related tools and agent communication

## Agent Groups Implementation ✅ COMPLETE (needs user testing)

The agent groups framework is now fully implemented! Groups allow multiple agents to work together using coordination patterns.

**⚠️ Testing Status**: Basic operations work and CLI commands function correctly. Overall integrity needs user testing to validate edge cases and real-world usage.

### What's Implemented

#### Phase 1: Configuration Structure ✅
- Added `GroupConfig` struct to `pattern_core/src/config.rs`
- Defined `GroupMemberConfig` with name, optional agent_id, role, and capabilities
- Integrated into main `PatternConfig` structure with groups vector

#### Phase 2: Database Operations ✅
- `create_group()` - Create a new agent group
- `create_group_for_user()` - Create group associated with user's constellation
- `get_group_by_name()` - Find group by name for a user
- `add_agent_to_group()` - Add an agent with a role and membership metadata
- `list_groups_for_user()` - List all groups owned by a user
- `get_group_members()` - Get all agents in a group with their roles
- Constellation operations for proper user->constellation->group relationships

#### Phase 3: CLI Commands ✅
- `pattern-cli group list` - Show all groups for current user
- `pattern-cli group create <name> -d <description> -p <pattern>` - Create a group
- `pattern-cli group add-member <group> <agent> --role <role>` - Add agent to group
- `pattern-cli group status <name>` - Show group details and members
- `pattern-cli chat --group <name>` - Chat with a group using its coordination pattern

**Note**: For multi-word descriptions in `just`, escape quotes: `just cli group create MyGroup --description \"My test group\"`
Or use the shortcut: `just group-create MyGroup "My test group"`

#### Phase 4: Runtime Integration ✅
- Group chat routes messages through coordination patterns (RoundRobin, Dynamic, Pipeline, etc.)
- Each agent in the group responds based on the pattern
- Supports all coordination patterns with proper manager instantiation
- Type-erased `dyn Agent` support for flexible group composition

### Still TODO

#### Phase 5: ADHD-Specific Templates
Create predefined group configurations in `pattern_nd`:
- **Main Group**: Round-robin between executive function agents
- **Crisis Group**: Dynamic selection based on urgency
- **Planning Group**: Pipeline pattern for task breakdown
- **Memory Group**: Supervisor pattern for memory management

#### Phase 6: Config Persistence
- Save groups to config file
- Load groups from config on startup
- Merge config groups with database groups

## Development Principles

- **Type Safety First**: Use Rust enums over string validation
- **Pure Rust**: Avoid C dependencies to reduce build complexity
- **Test-Driven**: All tests must validate actual behavior and be able to fail
- **Entity Relationships**: Use SurrealDB RELATE for all associations, no foreign keys
- **Atomic Operations**: Database operations are non-blocking with optimistic updates

## Workspace Structure

```
pattern/
├── crates/
│   ├── pattern_cli/      # Command-line testing tool
│   ├── pattern_core/     # Agent framework, memory, tools, coordination
│   ├── pattern_nd/       # Tools and agent personalities specific to the neurodivergent support constellation
│   ├── pattern_mcp/      # MCP server implementation
│   ├── pattern_discord/  # Discord bot integration
│   └── pattern_main/     # Main orchestrator binary (mostly legacy as of yet)
├── docs/                 # Architecture and integration guides
```

**Each crate has its own `CLAUDE.md` with specific implementation guidelines.**


## Core Architecture

### Agent Framework
- **DatabaseAgent**: Generic over ModelProvider and EmbeddingProvider
- **Built-in tools**: context, recall, search, send_message
- **Message persistence**: RELATE edges with Snowflake ID ordering
- **Memory system**: Thread-safe with semantic search, archival support, and atomic updates

### Coordination Patterns
- **Dynamic**: Selector-based routing (random, capability, load-balancing)
- **Round-robin**: Fair distribution with skip-inactive support
- **Sleeptime**: Background monitoring with intervention triggers
- **Pipeline**: Sequential processing through agent stages

### Entity System
Uses `#[derive(Entity)]` macro for SurrealDB integration:

```rust
#[derive(Entity)]
#[entity(entity_type = "user")]
pub struct User {
    pub id: UserId,
    pub username: String,

    // Relations via RELATE, not foreign keys
    #[entity(relation = "owns")]
    pub owned_agents: Vec<Agent>,
}
```

## Feature Development Workflow

1. **Branch Creation**: `git checkout -b feature/task-management`
2. **Implementation**: Follow crate-specific CLAUDE.md guidelines
3. **Testing**: Add tests that validate actual behavior
4. **Validation**: Run `just pre-commit-all` before commit
5. **PR**: Create pull request with clear description

## Current TODO List

### High Priority
- [X] Implement message compression with archival - COMPLETE
- [X] Add live query support for agent stats
- [X] Build agent groups framework
- [X] Create basic binary (CLI/TUI) for user testing - COMPLETE

### Medium Priority
- [X] Make agent groups usable via CLI and config system - COMPLETE
- [ ] Complete pattern-specific agent groups implementation (main, crisis, planning, memory)
- [ ] Implement task CRUD operations in pattern-core or pattern-nd
- [ ] Create ADHD-aware task manager with breakdown (pattern-nd)
- [ ] Add task-related MCP tools (create, update, list, breakdown)
- [ ] Add Discord context tools to MCP
- [ ] Implement time tracking with ADHD multipliers
- [ ] Add energy/attention monitoring
- [ ] Add vector search for archival memory using embeddings

### Documentation
Each major component has dedicated docs in `docs/`:
- **Architecture**: System design and component interactions
- **Guides**: Integration and setup instructions
- **API**: Common patterns and gotchas
- **Troubleshooting**: Known issues and solutions

## Build Commands

```bash
# Quick validation
cargo check
cargo test --lib

# Full pipeline (required before commit)
just pre-commit-all

# Development helpers
just watch                    # Auto-recompile on changes
cargo test --lib -- db::     # Run specific module tests
```

## Partner-Centric Architecture

Pattern uses a partner-centric model ensuring privacy:
- **Partner**: Person receiving ADHD support (owns constellation)
- **Conversant**: Someone interacting through partner's agents
- **Privacy**: DM content never bleeds into public channels
- **Scaling**: Each partner gets full constellation, hibernated when inactive

## References

- [MCP Rust SDK](https://github.com/modelcontextprotocol/rust-sdk)
- [MemGPT Paper](https://arxiv.org/abs/2310.08560) - Stateful agent architecture
- [SurrealDB Documentation](https://surrealdb.com/docs) - Graph database patterns
