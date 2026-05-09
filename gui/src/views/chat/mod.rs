mod chat_impl;
pub mod types;

pub use chat_impl::ChatView;
#[allow(unused_imports)]
pub use types::{
    AiStatus, Attachment, GenerationState, Message, PendingResponse, PhaseRecord, PromptTemplate,
    Session,
};
