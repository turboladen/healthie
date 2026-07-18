//! LLM-facing tool input schemas. Deliberately separate structs from
//! `healthie_shared::inputs` — the MCP shape (doc-commented for the model,
//! schemars-derived) is decoupled from the persistence inputs; each maps over
//! via `into_domain()`. Vocabulary enums come straight from the domain
//! (`schemars` feature on healthie-shared) so schemas can never drift.
