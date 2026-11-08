use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use typeshare::typeshare;
use utoipa::ToSchema;

/// Only database results and server-side account creation construct an owner.
/// Financial HTTP inputs deliberately cannot deserialize this type.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, ToSchema)]
#[serde(transparent)]
#[schema(value_type = String)]
pub struct UserId(String);

impl UserId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub(crate) fn from_database(value: String) -> Self {
        Self(value)
    }
}

#[typeshare]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Member,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}

impl FromStr for Role {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "admin" => Ok(Self::Admin),
            "member" => Ok(Self::Member),
            _ => Err(()),
        }
    }
}

#[typeshare]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AccountStatus {
    Pending,
    Active,
    Disabled,
}

impl AccountStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }
}

impl FromStr for AccountStatus {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "active" => Ok(Self::Active),
            "disabled" => Ok(Self::Disabled),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialKind {
    BrowserSession,
    PersonalReadToken,
    PersonalWriteToken,
}

#[derive(Clone, Debug)]
pub struct CurrentUser {
    pub id: UserId,
    pub username: String,
    pub role: Role,
    pub auth_version: i64,
    pub credential: CredentialKind,
}

#[typeshare]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionStage {
    Anonymous,
    MfaRequired,
    EnrollmentRequired,
    Authenticated,
}

impl SessionStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anonymous => "anonymous",
            Self::MfaRequired => "mfa_required",
            Self::EnrollmentRequired => "enrollment_required",
            Self::Authenticated => "authenticated",
        }
    }
}

impl FromStr for SessionStage {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "anonymous" => Ok(Self::Anonymous),
            "mfa_required" => Ok(Self::MfaRequired),
            "enrollment_required" => Ok(Self::EnrollmentRequired),
            "authenticated" => Ok(Self::Authenticated),
            _ => Err(()),
        }
    }
}

// Credential-bearing records intentionally implement neither Debug nor Serialize.
pub struct AuthUser {
    pub id: UserId,
    pub username: Option<String>,
    pub password_hash: Option<String>,
    pub role: Role,
    pub status: AccountStatus,
    pub totp_secret_encrypted: Option<String>,
    pub last_totp_step: Option<i64>,
    pub auth_version: i64,
    pub created_at: DateTime<Utc>,
}

impl AuthUser {
    pub fn fully_enrolled(&self) -> bool {
        self.status == AccountStatus::Active
            && self.username.is_some()
            && self.password_hash.is_some()
            && self.totp_secret_encrypted.is_some()
    }

    pub fn current(&self, credential: CredentialKind) -> Option<CurrentUser> {
        self.fully_enrolled().then(|| CurrentUser {
            id: self.id.clone(),
            username: self.username.clone().expect("enrolled username"),
            role: self.role,
            auth_version: self.auth_version,
            credential,
        })
    }
}

#[derive(Clone)]
pub struct PendingTotp {
    pub id: String,
    pub secret_encrypted: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct AuthSession {
    pub id_hash: String,
    pub stage: SessionStage,
    pub revision: i64,
    pub user_id: Option<UserId>,
    pub auth_version: Option<i64>,
    pub csrf_token: String,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub absolute_expires_at: DateTime<Utc>,
    pub reauthenticated_at: Option<DateTime<Utc>>,
    pub failed_factor_attempts: i32,
    pub pending_totp: Option<PendingTotp>,
    pub link_id: Option<String>,
    pub link_bound_until: Option<DateTime<Utc>>,
}

#[typeshare]
#[derive(Clone, Serialize, ToSchema)]
pub struct AuthUserView {
    pub id: String,
    pub username: String,
    pub role: Role,
    #[typeshare(serialized_as = "number")]
    pub auth_version: i64,
}

#[typeshare]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccountLinkKind {
    Bootstrap,
    Invite,
    PasswordReset,
    OperatorRecovery,
}

impl AccountLinkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bootstrap => "bootstrap",
            Self::Invite => "invite",
            Self::PasswordReset => "password_reset",
            Self::OperatorRecovery => "operator_recovery",
        }
    }
}

impl FromStr for AccountLinkKind {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "bootstrap" => Ok(Self::Bootstrap),
            "invite" => Ok(Self::Invite),
            "password_reset" => Ok(Self::PasswordReset),
            "operator_recovery" => Ok(Self::OperatorRecovery),
            _ => Err(()),
        }
    }
}

pub struct AuthLink {
    pub id: String,
    pub kind: AccountLinkKind,
    pub user_id: Option<UserId>,
    pub target_auth_version: Option<i64>,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[typeshare]
#[derive(Clone, Serialize, ToSchema)]
pub struct AccountLinkView {
    pub id: String,
    pub kind: AccountLinkKind,
    pub username: Option<String>,
    pub expires_at: DateTime<Utc>,
}

#[typeshare]
#[derive(Clone, Serialize, ToSchema)]
pub struct AuthState {
    pub stage: SessionStage,
    pub user: Option<AuthUserView>,
    pub csrf_token: String,
    pub expires_at: DateTime<Utc>,
    pub setup_required: bool,
    pub link: Option<AccountLinkView>,
}

#[typeshare]
#[derive(Serialize, ToSchema)]
pub struct UserSummary {
    pub id: String,
    pub username: Option<String>,
    pub role: Role,
    pub status: AccountStatus,
    pub totp_enabled: bool,
    pub created_at: DateTime<Utc>,
}

impl From<&AuthUser> for UserSummary {
    fn from(user: &AuthUser) -> Self {
        Self {
            id: user.id.as_str().to_owned(),
            username: user.username.clone(),
            role: user.role,
            status: user.status,
            totp_enabled: user.totp_secret_encrypted.is_some(),
            created_at: user.created_at,
        }
    }
}

#[typeshare]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ApiTokenScope {
    Read,
    Write,
}

impl ApiTokenScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

#[typeshare]
#[derive(Serialize, ToSchema)]
pub struct SessionSummary {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub current: bool,
}

#[typeshare]
#[derive(Serialize, ToSchema)]
pub struct ApiTokenSummary {
    pub id: String,
    pub name: String,
    pub scope: ApiTokenScope,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}
