//! `radivine-core` — Context model, RFC 2483 URI list parser, `Action` trait, `ActionRegistry`,
//! menu builder, and asynchronous `Dispatcher` (`ACTION_SYSTEM.md`).

#![forbid(unsafe_code)]

pub mod action;
pub mod context;
pub mod dispatch;
pub mod enrich;
pub mod registry;
pub mod test_util;

pub use action::{
    Action, ActionError, ActionId, ActionMeta, ClipOp, ClipboardRequest, Effect, Enablement,
    JobHandle, JobId, LogAction, Outcome,
};
pub use context::{
    BasicContext, Context, ContextKind, FileFacts, FileKind, InvocationMode, Item,
    MAX_CONTEXT_ITEMS, SourceInfo, TextEncoding, TextVerdict, local_path_to_file_uri,
    normalize_cli_arg_to_uri, parse_uri_list,
};
pub use dispatch::{Dispatcher, JobEvent, JobSink};
pub use enrich::{
    DEFAULT_ENRICHMENT_TIMEOUT, EnrichEvent, EnrichSink, Enricher, ItemInspector,
    MAX_ENRICHED_ITEMS, SNIFF_BUFFER_BYTES, enrich_context_sync, enrich_item, sniff_bytes,
    sniff_magic_mime,
};
pub use radivine_wheel::{IconKey, ItemPath, MenuNode, MenuTree, NodeKind};
pub use registry::{
    ActionIndex, ActionRegistry, EnablementUpdate, MenuLimits, MenuOverrides, MenuSpec,
    RegistryError,
};
