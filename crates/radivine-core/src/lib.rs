//! `radivine-core` — Context model, RFC 2483 URI list parser, `Action` trait, `ActionRegistry`,
//! menu builder, and asynchronous `Dispatcher` (`ACTION_SYSTEM.md`).

#![forbid(unsafe_code)]

pub mod action;
pub mod context;
pub mod dispatch;
pub mod registry;

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
pub use registry::{
    ActionIndex, ActionRegistry, EnablementUpdate, MenuLimits, MenuOverrides, MenuSpec,
    RegistryError,
};
