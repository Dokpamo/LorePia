pub(super) struct StoredGenerationRoute {
    pub(super) conversation: String,
    pub(super) branch: String,
    pub(super) user_message: String,
    pub(super) assistant_message: Option<String>,
    pub(super) provider_family: Option<super::ApiFamily>,
}
