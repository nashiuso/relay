//! The central client of Relay.
//!
//! The client owns the configured providers and implements the behaviour that
//! is shared by every backend: model resolution, bounded retries, opt-in
//! fallback and a small cache for model listings.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::error::{ErrorKind, RelayError};
use crate::model::{ModelInfo, ModelRef};
use crate::providers::traits::{ChatProvider, ProviderKind};
use crate::stream::ChatStream;
use crate::types::{ChatRequest, ChatResponse};

/// How long a model listing is cached.
pub const MODEL_CACHE_TTL: Duration = Duration::from_secs(300);

/// A summary of a configured provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSummary {
    /// The name the provider is configured under.
    pub name: String,
    /// The family the provider belongs to.
    pub kind: ProviderKind,
    /// A human readable description of the endpoint.
    pub description: String,
    /// The default model of the provider, when configured.
    pub default_model: Option<String>,
    /// Whether the provider is configured with credentials.
    pub has_credentials: bool,
}

/// A client that talks to the configured providers.
#[derive(Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    providers: BTreeMap<String, Arc<dyn ChatProvider>>,
    default_model: Option<ModelRef>,
    max_retries: u32,
    retry_backoff: Duration,
    fallback_models: Vec<ModelRef>,
    fallback_enabled: bool,
    model_cache: Mutex<HashMap<String, (Instant, Vec<ModelInfo>)>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field(
                "providers",
                &self.inner.providers.keys().collect::<Vec<_>>(),
            )
            .field("default_model", &self.inner.default_model)
            .finish()
    }
}

impl Client {
    /// Builds a client from the configuration file and the environment.
    pub fn from_config() -> Result<Client, RelayError> {
        Client::from_settings(Config::load()?)
    }

    /// Builds a client from an explicit configuration.
    pub fn from_settings(config: Config) -> Result<Client, RelayError> {
        let mut builder = Client::builder()
            .max_retries(config.max_retries)
            .retry_backoff(config.retry_backoff);
        for (name, provider_config) in &config.providers {
            builder = builder.provider(name, provider_config.build(name, config.timeout)?);
        }
        if let Some(model) = &config.default_model {
            builder = builder.default_model(model.clone());
        }
        builder = builder.fallback(config.fallback.enabled, config.fallback.models.clone());
        builder.build()
    }

    /// Creates a client builder.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// The names of the configured providers.
    pub fn provider_names(&self) -> Vec<String> {
        self.inner.providers.keys().cloned().collect()
    }

    /// A summary of every configured provider.
    pub fn providers(&self) -> Vec<ProviderSummary> {
        self.inner
            .providers
            .iter()
            .map(|(name, provider)| ProviderSummary {
                name: name.clone(),
                kind: provider.kind(),
                description: provider.describe(),
                default_model: provider.default_model().map(str::to_owned),
                has_credentials: provider.has_credentials(),
            })
            .collect()
    }

    /// The default model, when one is configured.
    pub fn default_model(&self) -> Option<&ModelRef> {
        self.inner.default_model.as_ref()
    }

    /// Lists the models of a provider.
    ///
    /// Results are cached for a few minutes so that repeated calls stay fast.
    /// Use [`Client::refresh_models`] to force a new request.
    pub fn models(&self, provider: &str) -> Result<Vec<ModelInfo>, RelayError> {
        if let Ok(cache) = self.inner.model_cache.lock() {
            if let Some((stamp, models)) = cache.get(provider) {
                if stamp.elapsed() < MODEL_CACHE_TTL {
                    return Ok(models.clone());
                }
            }
        }
        self.refresh_models(provider)
    }

    /// Lists the models of a provider, bypassing the cache.
    pub fn refresh_models(&self, provider: &str) -> Result<Vec<ModelInfo>, RelayError> {
        let provider = self.provider(provider)?;
        let models = provider.list_models()?;
        if let Ok(mut cache) = self.inner.model_cache.lock() {
            cache.insert(provider.name().to_owned(), (Instant::now(), models.clone()));
        }
        Ok(models)
    }

    /// Sends a chat request and waits for the complete answer.
    ///
    /// This is the blocking core of the client. [`Client::complete`] is the
    /// asynchronous facade around it and is the recommended entry point.
    pub fn complete_blocking(&self, request: ChatRequest) -> Result<ChatResponse, RelayError> {
        let candidates = self.candidates(&request);
        let mut last_error: Option<RelayError> = None;

        for (index, candidate) in candidates.iter().enumerate() {
            let attempt_request = request.clone().with_model(candidate.clone());
            match self.complete_with_retries(&attempt_request) {
                Ok(response) => return Ok(response),
                Err(err) => {
                    let is_last = index + 1 == candidates.len();
                    if is_last || !err.is_retriable() {
                        return Err(err);
                    }
                    last_error = Some(err);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            RelayError::new(ErrorKind::UnknownProvider, "no provider is configured")
        }))
    }

    /// Sends a chat request and waits for the complete answer.
    ///
    /// The request runs on a worker thread, so awaiting it never blocks the
    /// executor the caller uses. The future can be awaited from any runtime,
    /// including one that has never seen a Tokio reactor.
    ///
    /// ```no_run
    /// use relay_rs::{ChatRequest, Client, Role};
    ///
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = Client::from_config()?;
    /// let request = ChatRequest::new("provider/model").message(Role::User, "Hello!");
    /// let response = client.complete(request).await?;
    /// println!("{}", response.text());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn complete(&self, request: ChatRequest) -> Result<ChatResponse, RelayError> {
        let (sender, mut receiver) = crate::sync::channel();
        let client = self.clone();
        std::thread::spawn(move || {
            let _ = sender.send(client.complete_blocking(request));
        });
        receiver.recv().await.unwrap_or_else(|| {
            Err(RelayError::new(
                ErrorKind::Cancelled,
                "the request was cancelled before it finished",
            ))
        })
    }

    /// Starts a streaming chat request.
    ///
    /// This is the blocking core of the client. [`Client::stream`] is the
    /// asynchronous facade around it.
    pub fn stream_blocking(&self, request: ChatRequest) -> Result<ChatStream, RelayError> {
        let candidates = self.candidates(&request);
        let mut last_error: Option<RelayError> = None;

        for (index, candidate) in candidates.iter().enumerate() {
            let attempt_request = request.clone().with_model(candidate.clone());
            match self.stream_with_retries(&attempt_request) {
                Ok(stream) => return Ok(stream),
                Err(err) => {
                    let is_last = index + 1 == candidates.len();
                    if is_last || !err.is_retriable() {
                        return Err(err);
                    }
                    last_error = Some(err);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            RelayError::new(ErrorKind::UnknownProvider, "no provider is configured")
        }))
    }

    /// Starts a streaming chat request.
    ///
    /// The stream is produced on a worker thread and can be awaited from any
    /// runtime. Dropping the returned stream stops the transfer.
    pub async fn stream(&self, request: ChatRequest) -> Result<ChatStream, RelayError> {
        let (sender, mut receiver) = crate::sync::channel();
        let client = self.clone();
        std::thread::spawn(move || {
            let _ = sender.send(client.stream_blocking(request));
        });
        receiver.recv().await.unwrap_or_else(|| {
            Err(RelayError::new(
                ErrorKind::Cancelled,
                "the request was cancelled before it finished",
            ))
        })
    }

    fn provider(&self, name: &str) -> Result<&Arc<dyn ChatProvider>, RelayError> {
        self.inner.providers.get(name).ok_or_else(|| {
            let known = self.provider_names();
            let hint = if known.is_empty() {
                "no providers are configured".to_owned()
            } else {
                format!("configured providers: {}", known.join(", "))
            };
            RelayError::new(
                ErrorKind::UnknownProvider,
                format!("unknown provider `{name}`; {hint}"),
            )
        })
    }

    /// The models that may serve the request, in order.
    fn candidates(&self, request: &ChatRequest) -> Vec<ModelRef> {
        let mut candidates = vec![request.model().clone()];
        if self.inner.fallback_enabled {
            for model in &self.inner.fallback_models {
                if !candidates.contains(model) {
                    candidates.push(model.clone());
                }
            }
        }
        candidates
    }

    fn complete_with_retries(&self, request: &ChatRequest) -> Result<ChatResponse, RelayError> {
        let mut attempt = 0u32;
        loop {
            let result = self.complete_once(request);
            match result {
                Ok(response) => return Ok(response),
                Err(err) => {
                    if attempt >= self.inner.max_retries || !err.is_retriable() {
                        return Err(err);
                    }
                    self.sleep_before_retry(attempt);
                    attempt += 1;
                }
            }
        }
    }

    fn stream_with_retries(&self, request: &ChatRequest) -> Result<ChatStream, RelayError> {
        let mut attempt = 0u32;
        loop {
            let result = self.stream_once(request);
            match result {
                Ok(stream) => return Ok(stream),
                Err(err) => {
                    if attempt >= self.inner.max_retries || !err.is_retriable() {
                        return Err(err);
                    }
                    self.sleep_before_retry(attempt);
                    attempt += 1;
                }
            }
        }
    }

    fn complete_once(&self, request: &ChatRequest) -> Result<ChatResponse, RelayError> {
        let provider = self.resolve_provider(request)?;
        provider.complete(request)
    }

    fn stream_once(&self, request: &ChatRequest) -> Result<ChatStream, RelayError> {
        let provider = self.resolve_provider(request)?;
        provider.stream(request)
    }

    /// Resolves the provider and the model of a request.
    fn resolve_provider(&self, request: &ChatRequest) -> Result<Arc<dyn ChatProvider>, RelayError> {
        let model = request.model();
        let name = if model.has_provider() {
            model.provider().to_owned()
        } else {
            self.inner
                .default_model
                .as_ref()
                .map(|default| default.provider().to_owned())
                .filter(|provider| !provider.is_empty())
                .ok_or_else(|| {
                    RelayError::new(
                        ErrorKind::UnknownProvider,
                        "the request does not name a provider and no default model is configured",
                    )
                })?
        };
        let provider = self.provider(&name)?.clone();
        if model.model().is_empty() && provider.default_model().is_none() {
            return Err(RelayError::new(
                ErrorKind::UnknownModel,
                format!("provider `{name}` does not define a default model"),
            ));
        }
        Ok(provider)
    }

    fn sleep_before_retry(&self, attempt: u32) {
        let factor = 1u32 << attempt.min(5);
        std::thread::sleep(self.inner.retry_backoff.saturating_mul(factor));
    }
}

/// A builder for [`Client`].
#[derive(Debug, Default)]
pub struct ClientBuilder {
    providers: BTreeMap<String, Arc<dyn ChatProvider>>,
    default_model: Option<ModelRef>,
    max_retries: Option<u32>,
    retry_backoff: Option<Duration>,
    fallback_enabled: bool,
    fallback_models: Vec<ModelRef>,
}

impl ClientBuilder {
    /// Registers a provider.
    pub fn provider(mut self, name: impl Into<String>, provider: Arc<dyn ChatProvider>) -> Self {
        self.providers.insert(name.into(), provider);
        self
    }

    /// Sets the default model.
    pub fn default_model(mut self, model: impl Into<ModelRef>) -> Self {
        self.default_model = Some(model.into());
        self
    }

    /// Sets the number of retries for transient failures.
    pub fn max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = Some(max_retries);
        self
    }

    /// Sets the backoff between retries.
    pub fn retry_backoff(mut self, backoff: Duration) -> Self {
        self.retry_backoff = Some(backoff);
        self
    }

    /// Configures the fallback behaviour.
    ///
    /// Fallback is disabled by default: sending a prompt to another provider
    /// has to be an explicit decision.
    pub fn fallback(mut self, enabled: bool, models: Vec<String>) -> Self {
        self.fallback_enabled = enabled;
        self.fallback_models = models
            .iter()
            .map(|model| ModelRef::from(model.as_str()))
            .collect();
        self
    }

    /// Builds the client.
    pub fn build(self) -> Result<Client, RelayError> {
        if self.providers.is_empty() {
            return Err(RelayError::config(
                "no providers are configured; add one to the configuration file",
            ));
        }
        Ok(Client {
            inner: Arc::new(ClientInner {
                providers: self.providers,
                default_model: self.default_model,
                max_retries: self
                    .max_retries
                    .unwrap_or(crate::config::DEFAULT_MAX_RETRIES),
                retry_backoff: self
                    .retry_backoff
                    .unwrap_or(crate::config::DEFAULT_RETRY_BACKOFF),
                fallback_models: self.fallback_models,
                fallback_enabled: self.fallback_enabled,
                model_cache: Mutex::new(HashMap::new()),
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::traits::Capabilities;
    use std::sync::Mutex as StdMutex;

    #[derive(Debug)]
    struct FakeProvider {
        name: String,
        calls: StdMutex<usize>,
        fail_with: Option<ErrorKind>,
    }

    impl FakeProvider {
        fn new(name: &str) -> Arc<FakeProvider> {
            Arc::new(FakeProvider {
                name: name.to_owned(),
                calls: StdMutex::new(0),
                fail_with: None,
            })
        }

        fn failing(name: &str, kind: ErrorKind) -> Arc<FakeProvider> {
            Arc::new(FakeProvider {
                name: name.to_owned(),
                calls: StdMutex::new(0),
                fail_with: Some(kind),
            })
        }

        fn calls(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }

    impl ChatProvider for FakeProvider {
        fn name(&self) -> &str {
            &self.name
        }

        fn kind(&self) -> ProviderKind {
            ProviderKind::Custom
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities::none()
        }

        fn complete(&self, request: &ChatRequest) -> Result<ChatResponse, RelayError> {
            *self.calls.lock().unwrap() += 1;
            if let Some(kind) = self.fail_with {
                return Err(RelayError::provider(kind, &self.name, "boom"));
            }
            Ok(ChatResponse::new(
                &self.name,
                request.model().model(),
                format!("answer from {}", self.name),
            ))
        }

        fn stream(&self, _request: &ChatRequest) -> Result<ChatStream, RelayError> {
            unimplemented!("not used in the client tests")
        }

        fn list_models(&self) -> Result<Vec<ModelInfo>, RelayError> {
            Ok(vec![ModelInfo::new("fake", &self.name)])
        }
    }

    fn client_with(providers: Vec<(&str, Arc<FakeProvider>)>) -> Client {
        let mut builder = Client::builder()
            .max_retries(0)
            .retry_backoff(Duration::from_millis(1));
        for (name, provider) in providers {
            builder = builder.provider(name, provider);
        }
        builder.build().unwrap()
    }

    #[test]
    fn uses_the_selected_provider() {
        let alpha = FakeProvider::new("alpha");
        let beta = FakeProvider::new("beta");
        let client = client_with(vec![("alpha", alpha.clone()), ("beta", beta.clone())]);

        let response = client
            .complete_blocking(ChatRequest::new("alpha/model").user("hi"))
            .unwrap();
        assert_eq!(response.provider(), "alpha");
        assert_eq!(alpha.calls(), 1);
        assert_eq!(beta.calls(), 0);
    }

    #[test]
    fn reports_unknown_providers() {
        let client = client_with(vec![("alpha", FakeProvider::new("alpha"))]);
        let error = client
            .complete_blocking(ChatRequest::new("missing/model").user("hi"))
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::UnknownProvider);
    }

    #[test]
    fn does_not_fall_back_by_default() {
        let alpha = FakeProvider::failing("alpha", ErrorKind::Server);
        let beta = FakeProvider::new("beta");
        let client = client_with(vec![("alpha", alpha.clone()), ("beta", beta.clone())]);

        let error = client
            .complete_blocking(ChatRequest::new("alpha/model").user("hi"))
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Server);
        assert_eq!(beta.calls(), 0);
    }

    #[test]
    fn falls_back_when_enabled() {
        let alpha = FakeProvider::failing("alpha", ErrorKind::Server);
        let beta = FakeProvider::new("beta");
        let builder = Client::builder()
            .max_retries(0)
            .retry_backoff(Duration::from_millis(1))
            .provider("alpha", alpha.clone())
            .provider("beta", beta.clone())
            .fallback(true, vec!["beta/model".to_owned()]);
        let client = builder.build().unwrap();

        let response = client
            .complete_blocking(ChatRequest::new("alpha/model").user("hi"))
            .unwrap();
        assert_eq!(response.provider(), "beta");
        assert_eq!(beta.calls(), 1);
    }

    #[test]
    fn never_falls_back_for_permanent_failures() {
        let alpha = FakeProvider::failing("alpha", ErrorKind::Auth);
        let beta = FakeProvider::new("beta");
        let builder = Client::builder()
            .max_retries(0)
            .retry_backoff(Duration::from_millis(1))
            .provider("alpha", alpha.clone())
            .provider("beta", beta.clone())
            .fallback(true, vec!["beta/model".to_owned()]);
        let client = builder.build().unwrap();

        let error = client
            .complete_blocking(ChatRequest::new("alpha/model").user("hi"))
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Auth);
        assert_eq!(beta.calls(), 0);
    }

    #[test]
    fn retries_transient_failures_a_bounded_number_of_times() {
        let alpha = FakeProvider::failing("alpha", ErrorKind::Server);
        let builder = Client::builder()
            .max_retries(2)
            .retry_backoff(Duration::from_millis(1))
            .provider("alpha", alpha.clone());
        let client = builder.build().unwrap();

        let error = client
            .complete_blocking(ChatRequest::new("alpha/model").user("hi"))
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Server);
        assert_eq!(alpha.calls(), 3);
    }

    #[test]
    fn does_not_retry_permanent_failures() {
        let alpha = FakeProvider::failing("alpha", ErrorKind::InvalidRequest);
        let builder = Client::builder()
            .max_retries(3)
            .retry_backoff(Duration::from_millis(1))
            .provider("alpha", alpha.clone());
        let client = builder.build().unwrap();

        let error = client
            .complete_blocking(ChatRequest::new("alpha/model").user("hi"))
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidRequest);
        assert_eq!(alpha.calls(), 1);
    }

    #[test]
    fn lists_and_caches_models() {
        let alpha = FakeProvider::new("alpha");
        let client = client_with(vec![("alpha", alpha.clone())]);
        let models = client.models("alpha").unwrap();
        assert_eq!(models.len(), 1);
        let models = client.models("alpha").unwrap();
        assert_eq!(models.len(), 1);
    }

    #[test]
    fn uses_the_default_provider_for_bare_models() {
        let alpha = FakeProvider::new("alpha");
        let builder = Client::builder()
            .max_retries(0)
            .provider("alpha", alpha.clone())
            .default_model("alpha/only");
        let client = builder.build().unwrap();

        let response = client
            .complete_blocking(ChatRequest::new("model").user("hi"))
            .unwrap();
        assert_eq!(response.provider(), "alpha");
    }
}
