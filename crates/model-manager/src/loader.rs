use std::path::Path;

pub trait ModelLoader: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    fn load(path: &Path) -> Result<Self, Self::Error>
    where
        Self: Sized;
}

#[cfg(feature = "cactus")]
impl ModelLoader for poha_cactus::Model {
    type Error = poha_cactus::Error;

    fn load(path: &Path) -> Result<Self, Self::Error> {
        poha_cactus::Model::new(path)
    }
}

#[cfg(feature = "whisper-local")]
impl ModelLoader for poha_whisper_local::LoadedWhisper {
    type Error = poha_whisper_local::Error;

    fn load(path: &Path) -> Result<Self, Self::Error> {
        poha_whisper_local::LoadedWhisper::builder()
            .model_path(path.to_string_lossy().into_owned())
            .build()
    }
}
