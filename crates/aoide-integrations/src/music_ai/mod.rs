//! The app's half of music by description, which Melete's AI answers:
//! whether a phrase is worth sending ([`phrase`]), and a mix's rules checked
//! again and resolved against the library ([`smart_rules`], [`mix_plan`]).
//!
//! **The model only translates.** Melete sends the words and the library's
//! genre names to OpenRouter with the server's own key, and answers with
//! filters or rules; the library and the local history select the tracks,
//! so a model can never put a song in front of the listener. Nothing here
//! sends anything.

mod json;
pub mod mix_plan;
pub mod phrase;
pub mod smart_rules;
