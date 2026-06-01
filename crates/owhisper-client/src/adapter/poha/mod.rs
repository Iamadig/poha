mod batch;
mod live;

#[derive(Clone, Default)]
pub struct PohaAdapter;

impl PohaAdapter {
    pub fn is_supported_languages_live(
        _languages: &[poha_language::Language],
        _model: Option<&str>,
    ) -> bool {
        true
    }

    pub fn is_supported_languages_batch(
        _languages: &[poha_language::Language],
        _model: Option<&str>,
    ) -> bool {
        true
    }
}
