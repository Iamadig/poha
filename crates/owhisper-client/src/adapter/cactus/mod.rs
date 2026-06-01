#[cfg(feature = "local")]
mod batch;
mod live;

#[cfg(feature = "local")]
mod retry;

#[derive(Clone, Default)]
pub struct CactusAdapter;

impl CactusAdapter {
    pub fn is_supported_languages_live(
        _languages: &[poha_language::Language],
        _model: Option<&str>,
    ) -> bool {
        false
    }
}
