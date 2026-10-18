//! Уникальные индексы — все методы `*_unique*` менеджера индексов.
//!
//! Реализация вынесена в отдельный файл для разделения ответственности:
//! этот модуль отвечает за гарантии уникальности значений.

mod create;
mod listing;
mod validate;

#[cfg(test)]
pub(crate) use validate::{PostFlagSetPreGenBumpHook, TEST_POST_FLAG_SET_PRE_GEN_BUMP_HOOK};
