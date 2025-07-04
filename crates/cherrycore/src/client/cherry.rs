use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::{
    Client, ClientBuilder,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use serde::{Deserialize, Serialize};
use streamstore::StreamId;
use uuid::Uuid;

use crate::types::{
    CheckAclRequest, CheckAclResponse, Contact, Conversation, CreateConversationRequest, CreateConversationResponse, ListConversationsResponse, ListStreamRequest, ListStreamResponse, LoginRequest, LoginResponse, ResponseError, User
};

use super::{ClientConfig, AuthCredentials};

/// Professional Cherry client implementation
#[derive(Clone)]
pub struct CherryClient {
    inner: Arc<CherryClientInner>,
}

#[derive(Clone)]
pub struct CherryClientInner {
    config: ClientConfig,
    client: Client,
    auth: Option<AuthCredentials>,
}

impl std::ops::Deref for CherryClient {
    type Target = CherryClientInner;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl CherryClient {
    /// Create a new client with default configuration
    pub fn new() -> Result<Self> {
        Self::new_with_config(ClientConfig::default_cherry())
    }

    /// Create a new client with custom configuration
    pub fn new_with_config(config: ClientConfig) -> Result<Self> {
        let client = ClientBuilder::new()
            .timeout(config.timeout)
            .pool_idle_timeout(config.pool_idle_timeout)
            .pool_max_idle_per_host(config.max_idle_per_host)
            .user_agent(config.user_agent.clone())
            .no_proxy()
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            inner: Arc::new(CherryClientInner {
                config,
                client,
                auth: None,
            }),
        })
    }

    pub fn new_with_base_url(base_url: String) -> Result<Self> {
        let config = ClientConfig {
            base_url,
            ..ClientConfig::default_cherry()
        };
        Self::new_with_config(config)
    }

    /// Set authentication credentials
    pub fn with_auth(self, auth: impl Into<AuthCredentials>) -> Self {
        let inner = CherryClientInner {
            auth: Some(auth.into()),
            client: self.inner.client.clone(),
            config: self.inner.config.clone(),
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Create authenticated headers
    fn create_headers(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();

        // Set content type
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        // Set authorization if available
        if let Some(auth) = &self.auth {
            let auth_value = HeaderValue::from_str(&format!("Bearer {}", auth.jwt_token))
                .context("Invalid JWT token format")?;
            headers.insert(AUTHORIZATION, auth_value);
        }

        Ok(headers)
    }

    /// Make an authenticated request
    async fn request<T, Q>(&self, method: reqwest::Method, endpoint: &str, query: Option<&Q>) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
        Q: Serialize,
    {
        let url = format!("{}{}", self.config.base_url, endpoint);
        let headers = self.create_headers()?;

        log::info!("request: url={}, headers={:?}", url, headers);

        let req = self.client.request(method, &url).headers(headers);
        let req = if let Some(q) = query {
            req.query(&q)
        } else {
            req
        };
        let response = req
            .send()
            .await
            .context("Request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow::anyhow!("HTTP {}: {}", status, error_text));
        }

        response
            .json::<T>()
            .await
            .context("Failed to deserialize response")
    }

    /// Make a POST request with JSON body
    async fn request_with_body<T, U>(
        &self,
        method: reqwest::Method,
        endpoint: &str,
        body: &T,
    ) -> Result<U>
    where
        T: Serialize,
        U: for<'de> Deserialize<'de>,
    {
        let url = format!("{}{}", self.config.base_url, endpoint);
        let headers = self.create_headers()?;

        log::info!("request: url={}, headers={:?}", url, headers);

        let response = self
            .client
            .request(method, &url)
            .headers(headers)
            .json(body)
            .send()
            .await
            .context("Request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow::anyhow!("HTTP {}: {}", status, error_text));
        }

        response
            .json::<U>()
            .await
            .context("Failed to deserialize response")
    }

    /// Login and get authentication credentials
    pub async fn login(&self, email: &str, password: &str) -> Result<LoginResponse> {
        let login_request = LoginRequest {
            email: Some(email.to_string()),
            password: Some(password.to_string()),
            type_: "email".to_string(),
        };

        let login_response = self
            .request_with_body::<LoginRequest, LoginResponse>(
                reqwest::Method::POST,
                "/api/v1/auth/login",
                &login_request,
            )
            .await?;

        Ok(login_response)
    }

    /// Get all contacts for the authenticated user
    pub async fn get_contacts(&self) -> Result<Vec<Contact>> {
        self.request::<Vec<Contact>, ()>(reqwest::Method::GET, "/api/v1/contract/list", None)
            .await
    }

    /// Get user by ID
    pub async fn get_user(&self, user_id: Uuid) -> Result<User> {
        self.request::<User, ()>(reqwest::Method::GET, &format!("/api/v1/users/{}", user_id), None)
            .await
    }

    pub async fn check_acl(&self, user_id: Uuid, stream_id: Option<StreamId>, conversation_id: Option<Uuid>) -> Result<bool> {
        let request = CheckAclRequest { user_id, stream_id, conversation_id };
        let response = self.request::<CheckAclResponse, CheckAclRequest>(reqwest::Method::GET, "/api/v1/acl/check", Some(&request)).await?;
        Ok(response.allowed)
    }

    /// Create a new conversation
    pub async fn create_conversation(&self, conversation_type: String, members: &[Uuid]) -> Result<Conversation> {
        let request = CreateConversationRequest {
            conversation_type,
            members: members.to_vec(),
            meta: None,
        };
        let response = self.request_with_body::<CreateConversationRequest, CreateConversationResponse>(reqwest::Method::POST, "/api/v1/conversations/create", &request).await?;
        Ok(Conversation {
            conversation_id: response.conversation_id,
            conversation_type: response.conversation_type,
            members: response.members.iter().map(|m| m.to_string()).collect::<Vec<String>>().into(),
            meta: response.meta,
            stream_id: response.stream_id,
            created_at: response.created_at,
            updated_at: response.created_at,
        })
    }

    /// Get all conversations for the authenticated user
    pub async fn get_conversations(&self) -> Result<Vec<Conversation>> {
        let response = self
            .request::<ListConversationsResponse, ()>(
                reqwest::Method::GET,
                "/api/v1/conversations/list",
                None,
            )
            .await?;
        Ok(response.conversations)
    }

    /// Get all streams for a user
    pub async fn get_streams(&self, user_id: Uuid) -> Result<ListStreamResponse> {
        let request = ListStreamRequest { user_id };

        let url = format!("{}/api/v1/streams/list", self.config.base_url);
        let headers = self.create_headers()?;

        let response = self
            .client
            .get(&url)
            .headers(headers)
            .query(&request)
            .send()
            .await
            .context("Failed to get streams")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow::anyhow!("HTTP {}: {}", status, error_text));
        }

        response
            .json::<ListStreamResponse>()
            .await
            .context("Failed to deserialize streams response")
    }
}

/// Builder pattern for creating CherryClient instances
pub struct CherryClientBuilder {
    config: ClientConfig,
    auth: Option<AuthCredentials>,
}

impl CherryClientBuilder {
    pub fn new() -> Self {
        Self {
            config: ClientConfig::default_cherry(),
            auth: None,
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.config.base_url = base_url;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.config.timeout = timeout;
        self
    }

    pub fn with_auth(mut self, auth: AuthCredentials) -> Self {
        self.auth = Some(auth);
        self
    }

    pub fn with_user_agent(mut self, user_agent: String) -> Self {
        self.config.user_agent = user_agent;
        self
    }

    pub fn build(self) -> Result<CherryClient> {
        let mut client = CherryClient::new_with_config(self.config)?;
        if let Some(auth) = self.auth {
            client = client.with_auth(auth);
        }
        Ok(client)
    }
}

impl Default for CherryClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}
