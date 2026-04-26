# OAuth 2.0 provider (API)

A complete OAuth 2.0 authorization server implementation for this project's HTTP API, following RFC 6749 specifications with PKCE support (RFC 7636) and modern security practices.

## Overview

This OAuth provider enables third-party applications to securely access user data through standardized OAuth 2.0 flows. It integrates seamlessly with this project's existing authentication system and provides a user-friendly authorization interface.

### Key Features

- **RFC 6749 Compliant**: Full OAuth 2.0 authorization code flow implementation
- **PKCE Support**: Enhanced security with Proof Key for Code Exchange (RFC 7636)
- **Secure Token Management**: Cryptographically secure token generation and validation
- **User-Friendly UI**: Clean authorization pages with clear permission descriptions
- **Comprehensive Error Handling**: Detailed error responses following OAuth specifications
- **Database Integration**: Persistent storage for clients, codes, and tokens
- **Session Integration**: Works with existing axum-login authentication system

## Quick Start

### 1. Database Setup

The OAuth system requires three database tables that are automatically created via migrations:

```sql
-- OAuth clients (applications)
oauth_clients
-- Authorization codes (temporary)
oauth_authorization_codes
-- Access tokens (long-lived)
oauth_access_tokens
```

### 2. Register an OAuth Client

Currently, OAuth clients must be registered directly in the database. Here's an example:

```rust
use crate::api::oauth::{db, utils, OAuthScope};

// Generate client credentials
let client_id = utils::generate_client_id();
let client_secret = utils::generate_client_secret();
let client_secret_hash = utils::hash_client_secret(&client_secret)?;

// Define allowed redirect URIs and scopes
let redirect_uris = serde_json::json!([
    "https://myapp.example.com/oauth/callback",
    "http://localhost:3000/callback"  // For development
]);
let scopes = vec![OAuthScope::Profile, OAuthScope::Email];

// Create the client
let client_db_id = db::create_oauth_client(
    &pool,
    &client_id,
    &client_secret_hash,
    "My Application",
    &redirect_uris,
    &scopes,
).await?;

println!("Client ID: {}", client_id);
println!("Client Secret: {}", client_secret);
```

### 3. Integration with Axum

Add the OAuth router to your Axum application:

```rust
use crate::api::oauth::router::create_oauth_router;
use crate::api::oauth::endpoints::OAuthState;

let oauth_state = OAuthState {
    pool: pool.clone(),
};

let app = Router::new()
    .nest("/oauth", create_oauth_router())
    .with_state(oauth_state);
```

## Client Registration

### Manual Registration

Currently, OAuth clients must be registered manually in the database. Each client requires:

- **Client ID**: Unique public identifier (24 characters)
- **Client Secret**: Private key for authentication (48 characters, hashed)
- **Name**: Human-readable application name
- **Redirect URIs**: JSON array of allowed callback URLs
- **Scopes**: JSON array of permitted OAuth scopes

### Client Credentials

```rust
// Generate secure credentials
let client_id = utils::generate_client_id();        // 24 chars
let client_secret = utils::generate_client_secret(); // 48 chars
let client_secret_hash = utils::hash_client_secret(&client_secret)?;
```

### Redirect URI Validation

Redirect URIs must meet security requirements:

- **HTTPS required** for production domains
- **HTTP allowed** only for localhost/127.0.0.1 (development)
- Must be exact matches (no wildcards)

```rust
// Valid redirect URIs
"https://myapp.com/callback"     // Production
"http://localhost:3000/callback" // Development
"http://127.0.0.1:8080/callback" // Development

// Invalid redirect URIs
"http://myapp.com/callback"      // HTTP not allowed for non-localhost
"ftp://example.com/callback"     // Invalid scheme
```

## OAuth Endpoints

### Authorization Endpoint

**`GET /oauth/authorize`**

Initiates the OAuth authorization flow. Requires user authentication.

#### Parameters

| Parameter | Required | Description |
|-----------|----------|-------------|
| `response_type` | Yes | Must be `"code"` |
| `client_id` | Yes | Client identifier |
| `redirect_uri` | Yes | Callback URL (must be registered) |
| `scope` | No | Space-separated scopes (e.g., `"profile email"`) |
| `state` | Recommended | CSRF protection token |
| `code_challenge` | No | PKCE code challenge |
| `code_challenge_method` | No | PKCE method (`"S256"` or `"plain"`) |

#### Example Request

```
GET /oauth/authorize?response_type=code&client_id=ABC123&redirect_uri=https%3A//myapp.com/callback&scope=profile%20email&state=xyz789&code_challenge=dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk&code_challenge_method=S256
```

#### Response

- **Success**: Authorization page rendered for user approval
- **Error**: Error page with detailed explanation
- **Redirect**: To login if user not authenticated

---

**`POST /oauth/authorize`**

Handles user's authorization decision (Allow/Deny).

#### Form Parameters

| Parameter | Required | Description |
|-----------|----------|-------------|
| `action` | Yes | `"allow"` or `"deny"` |
| `csrf_token` | Yes | CSRF protection |
| All original OAuth parameters | Yes | Repeated from GET request |

#### Response

- **Allow**: Redirect to `redirect_uri` with authorization code
- **Deny**: Redirect to `redirect_uri` with error

### Token Endpoint

**`POST /oauth/token`**

Exchanges authorization code for access token.

#### Parameters (Form-encoded)

| Parameter | Required | Description |
|-----------|----------|-------------|
| `grant_type` | Yes | Must be `"authorization_code"` |
| `code` | Yes | Authorization code from authorize endpoint |
| `redirect_uri` | Yes | Same URI used in authorization request |
| `client_id` | Yes | Client identifier |
| `client_secret` | Conditional | Required unless using PKCE |
| `code_verifier` | No | PKCE code verifier |

#### Example Request

```bash
curl -X POST https://api.newapp.example/oauth/token \
  -H "Content-Type: application/x-www-form-urlencoded" \
  -d "grant_type=authorization_code" \
  -d "code=AUTH_CODE_HERE" \
  -d "redirect_uri=https://myapp.com/callback" \
  -d "client_id=ABC123" \
  -d "client_secret=SECRET_HERE"
```

#### Response

```json
{
  "access_token": "ACCESS_TOKEN_HERE",
  "token_type": "Bearer",
  "expires_in": 3600,
  "scope": "profile email"
}
```

### User Info Endpoint

**`GET /oauth/userinfo`**

Returns user information based on token scopes.

#### Authentication

Requires Bearer token in Authorization header:

```
Authorization: Bearer ACCESS_TOKEN_HERE
```

#### Example Request

```bash
curl -X GET https://api.newapp.example/oauth/userinfo \
  -H "Authorization: Bearer ACCESS_TOKEN_HERE"
```

#### Response

```json
{
  "sub": "123",
  "preferred_username": "johndoe",
  "name": "John Doe",
  "email": "john@example.com",
  "email_verified": false
}
```

## Supported Scopes

The OAuth provider supports four scopes following a consistent resource:action naming pattern:

### `profile:read`
- **Description**: Read access to basic profile information
- **Grants access to**:
  - `sub` (user ID)
  - `preferred_username`
  - `name` (display name)
- **Required for**: All authenticated access

### `profile:email`
- **Description**: Read access to user's email address
- **Grants access to**:
  - `email`
  - `email_verified`
- **Requires**: `profile:read`

### `data:read`
- **Description**: Placeholder for read access to resources your API exposes (rename/split scopes for your product).
- **Grants access to**: Whatever your handlers check for this scope (this starter does not ship domain-specific APIs).
- **Requires**: `profile:read`

### `data:write`
- **Description**: Placeholder for write access to resources your API exposes (rename/split scopes for your product).
- **Grants access to**: Whatever your handlers check for this scope (this starter does not ship domain-specific APIs).
- **Requires**: `profile:read`

### Scope Usage

```javascript
// Request multiple scopes using resource:action format
const authUrl = `https://api.newapp.example/oauth/authorize?` +
  `response_type=code&` +
  `client_id=YOUR_CLIENT_ID&` +
  `redirect_uri=${encodeURIComponent(redirectUri)}&` +
  `scope=profile:read data:read data:write&` +
  `state=${state}`;
```

#### Common Scope Combinations

```javascript
// Basic profile access (minimum for authentication)
const basicScopes = ["profile:read"];

// Profile with email access
const profileWithEmail = ["profile:read", "profile:email"];

// Example third-party viewer application
const viewerApp = ["profile:read", "data:read"];

// Complete tracking application
const trackingApp = ["profile:read", "data:read", "data:write"];

// Full access application
const fullAccess = ["profile:read", "profile:email", "data:read", "data:write"];
```

## Authorization Flow

### Step-by-Step OAuth 2.0 Flow

#### 1. Authorization Request

Client redirects user to authorization endpoint:

```javascript
const authUrl = `https://api.newapp.example/oauth/authorize?` +
  `response_type=code&` +
  `client_id=YOUR_CLIENT_ID&` +
  `redirect_uri=${encodeURIComponent('https://myapp.com/callback')}&` +
  `scope=profile email&` +
  `state=RANDOM_STATE_VALUE`;

window.location.href = authUrl;
```

#### 2. User Authentication & Authorization

- User is redirected to login if not authenticated
- After login, user sees authorization page
- User clicks "Allow" or "Deny"

#### 3. Authorization Response

**Success (user clicked "Allow"):**
```
https://myapp.com/callback?code=AUTHORIZATION_CODE&state=RANDOM_STATE_VALUE
```

**Error (user clicked "Deny"):**
```
https://myapp.com/callback?error=access_denied&error_description=The+resource+owner+denied+the+request&state=RANDOM_STATE_VALUE
```

#### 4. Token Exchange

Client exchanges authorization code for access token:

```javascript
const response = await fetch('https://api.newapp.example/oauth/token', {
  method: 'POST',
  headers: {
    'Content-Type': 'application/x-www-form-urlencoded',
  },
  body: new URLSearchParams({
    grant_type: 'authorization_code',
    code: authorizationCode,
    redirect_uri: 'https://myapp.com/callback',
    client_id: 'YOUR_CLIENT_ID',
    client_secret: 'YOUR_CLIENT_SECRET',
  }),
});

const tokenData = await response.json();
// tokenData.access_token contains the access token
```

#### 5. API Access

Use access token to make authenticated requests:

```javascript
const userInfo = await fetch('https://api.newapp.example/oauth/userinfo', {
  headers: {
    'Authorization': `Bearer ${accessToken}`,
  },
});

const userData = await userInfo.json();
```

## PKCE Support

PKCE (Proof Key for Code Exchange) provides enhanced security, especially for public clients.

### PKCE Flow

#### 1. Generate Code Verifier and Challenge

```javascript
// Generate code verifier (43-128 characters)
function generateCodeVerifier() {
  const array = new Uint8Array(32);
  crypto.getRandomValues(array);
  return btoa(String.fromCharCode.apply(null, array))
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=/g, '');
}

// Generate code challenge (SHA256 hash of verifier)
async function generateCodeChallenge(verifier) {
  const encoder = new TextEncoder();
  const data = encoder.encode(verifier);
  const digest = await crypto.subtle.digest('SHA-256', data);
  return btoa(String.fromCharCode.apply(null, new Uint8Array(digest)))
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=/g, '');
}

const codeVerifier = generateCodeVerifier();
const codeChallenge = await generateCodeChallenge(codeVerifier);
```

#### 2. Authorization Request with PKCE

```javascript
const authUrl = `https://api.newapp.example/oauth/authorize?` +
  `response_type=code&` +
  `client_id=YOUR_CLIENT_ID&` +
  `redirect_uri=${encodeURIComponent(redirectUri)}&` +
  `scope=profile email&` +
  `state=${state}&` +
  `code_challenge=${codeChallenge}&` +
  `code_challenge_method=S256`;
```

#### 3. Token Exchange with PKCE

```javascript
const response = await fetch('https://api.newapp.example/oauth/token', {
  method: 'POST',
  headers: {
    'Content-Type': 'application/x-www-form-urlencoded',
  },
  body: new URLSearchParams({
    grant_type: 'authorization_code',
    code: authorizationCode,
    redirect_uri: redirectUri,
    client_id: 'YOUR_CLIENT_ID',
    code_verifier: codeVerifier, // Instead of client_secret
  }),
});
```

### PKCE Benefits

- **Enhanced Security**: Prevents authorization code interception attacks
- **Public Client Support**: Enables secure OAuth for SPAs and mobile apps
- **No Client Secret**: Eliminates need to store secrets in public clients

## API Usage Examples

### Complete JavaScript Example

```javascript
class ExampleOAuthClient {
  constructor(clientId, redirectUri) {
    this.clientId = clientId;
    this.redirectUri = redirectUri;
    this.baseUrl = 'https://api.newapp.example';
  }

  // Generate PKCE parameters
  async generatePKCE() {
    const codeVerifier = this.generateCodeVerifier();
    const codeChallenge = await this.generateCodeChallenge(codeVerifier);

    return { codeVerifier, codeChallenge };
  }

  generateCodeVerifier() {
    const array = new Uint8Array(32);
    crypto.getRandomValues(array);
    return btoa(String.fromCharCode.apply(null, array))
      .replace(/\+/g, '-')
      .replace(/\//g, '_')
      .replace(/=/g, '');
  }

  async generateCodeChallenge(verifier) {
    const encoder = new TextEncoder();
    const data = encoder.encode(verifier);
    const digest = await crypto.subtle.digest('SHA-256', data);
    return btoa(String.fromCharCode.apply(null, new Uint8Array(digest)))
      .replace(/\+/g, '-')
      .replace(/\//g, '_')
      .replace(/=/g, '');
  }

  // Start OAuth flow
  async authorize(scopes = ['profile', 'email']) {
    const { codeVerifier, codeChallenge } = await this.generatePKCE();
    const state = crypto.randomUUID();

    // Store for later use
    sessionStorage.setItem('oauth_code_verifier', codeVerifier);
    sessionStorage.setItem('oauth_state', state);

    const authUrl = `${this.baseUrl}/oauth/authorize?` +
      `response_type=code&` +
      `client_id=${this.clientId}&` +
      `redirect_uri=${encodeURIComponent(this.redirectUri)}&` +
      `scope=${encodeURIComponent(scopes.join(' '))}&` +
      `state=${state}&` +
      `code_challenge=${codeChallenge}&` +
      `code_challenge_method=S256`;

    window.location.href = authUrl;
  }

  // Handle callback and exchange code for token
  async handleCallback() {
    const urlParams = new URLSearchParams(window.location.search);
    const code = urlParams.get('code');
    const state = urlParams.get('state');
    const error = urlParams.get('error');

    if (error) {
      throw new Error(`OAuth error: ${error}`);
    }

    if (!code) {
      throw new Error('No authorization code received');
    }

    // Verify state
    const storedState = sessionStorage.getItem('oauth_state');
    if (state !== storedState) {
      throw new Error('Invalid state parameter');
    }

    const codeVerifier = sessionStorage.getItem('oauth_code_verifier');
    if (!codeVerifier) {
      throw new Error('No code verifier found');
    }

    // Exchange code for token
    const response = await fetch(`${this.baseUrl}/oauth/token`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/x-www-form-urlencoded',
      },
      body: new URLSearchParams({
        grant_type: 'authorization_code',
        code: code,
        redirect_uri: this.redirectUri,
        client_id: this.clientId,
        code_verifier: codeVerifier,
      }),
    });

    if (!response.ok) {
      const errorData = await response.json();
      throw new Error(`Token exchange failed: ${errorData.error_description}`);
    }

    const tokenData = await response.json();

    // Store access token
    localStorage.setItem('oauth_access_token', tokenData.access_token);

    // Clean up session storage
    sessionStorage.removeItem('oauth_code_verifier');
    sessionStorage.removeItem('oauth_state');

    return tokenData;
  }

  // Get user information
  async getUserInfo() {
    const accessToken = localStorage.getItem('oauth_access_token');
    if (!accessToken) {
      throw new Error('No access token available');
    }

    const response = await fetch(`${this.baseUrl}/oauth/userinfo`, {
      headers: {
        'Authorization': `Bearer ${accessToken}`,
      },
    });

    if (!response.ok) {
      if (response.status === 401) {
        // Token expired or invalid
        localStorage.removeItem('oauth_access_token');
        throw new Error('Access token expired');
      }
      throw new Error(`Failed to get user info: ${response.status}`);
    }

    return await response.json();
  }
}

// Usage
const client = new ExampleOAuthClient('YOUR_CLIENT_ID', 'https://myapp.com/callback');

// Start authorization
await client.authorize(['profile', 'email']);

// In callback page
try {
  const tokenData = await client.handleCallback();
  console.log('Access token:', tokenData.access_token);

  const userInfo = await client.getUserInfo();
  console.log('User info:', userInfo);
} catch (error) {
  console.error('OAuth error:', error.message);
}
```

### cURL Examples

#### Authorization Code Exchange

```bash
curl -X POST https://api.newapp.example/oauth/token \
  -H "Content-Type: application/x-www-form-urlencoded" \
  -d "grant_type=authorization_code" \
  -d "code=AUTHORIZATION_CODE_HERE" \
  -d "redirect_uri=https://myapp.com/callback" \
  -d "client_id=YOUR_CLIENT_ID" \
  -d "client_secret=YOUR_CLIENT_SECRET"
```

#### User Info Request

```bash
curl -X GET https://api.newapp.example/oauth/userinfo \
  -H "Authorization: Bearer ACCESS_TOKEN_HERE"
```

#### Token Validation

```bash
# The userinfo endpoint can be used to validate tokens
curl -X GET https://api.newapp.example/oauth/userinfo \
  -H "Authorization: Bearer ACCESS_TOKEN_HERE" \
  -w "%{http_code}"
```

## Error Handling

### OAuth Error Responses

All OAuth errors follow RFC 6749 format:

```json
{
  "error": "error_code",
  "error_description": "Human readable description"
}
```

### Common Error Codes

#### Authorization Endpoint Errors

| Error Code | Description | Common Causes |
|------------|-------------|---------------|
| `invalid_request` | Malformed request | Missing required parameters, invalid URL |
| `unauthorized_client` | Client not authorized | Client not registered or inactive |
| `access_denied` | User denied access | User clicked "Deny" |
| `unsupported_response_type` | Invalid response type | response_type not "code" |
| `invalid_scope` | Invalid scope requested | Unknown scope or scope not allowed for client |
| `server_error` | Internal server error | Database error, system failure |

#### Token Endpoint Errors

| Error Code | Description | Common Causes |
|------------|-------------|---------------|
| `invalid_request` | Malformed request | Missing parameters, invalid format |
| `invalid_client` | Client authentication failed | Wrong client_id/secret, client not found |
| `invalid_grant` | Invalid authorization code | Code expired, already used, or doesn't exist |
| `unsupported_grant_type` | Invalid grant type | grant_type not "authorization_code" |

#### User Info Endpoint Errors

| Error Code | HTTP Status | Description |
|------------|-------------|-------------|
| `invalid_token` | 401 | Token expired, revoked, or malformed |
| `insufficient_scope` | 403 | Token lacks required scopes |

### Error Handling Best Practices

#### Client-Side Error Handling

```javascript
async function handleOAuthError(response) {
  if (!response.ok) {
    const errorData = await response.json();

    switch (errorData.error) {
      case 'invalid_client':
        console.error('Client authentication failed - check credentials');
        break;
      case 'invalid_grant':
        console.error('Authorization code expired or invalid - restart flow');
        break;
      case 'access_denied':
        console.log('User denied access');
        break;
      case 'invalid_token':
        console.error('Access token expired - need to re-authorize');
        // Clear stored token and redirect to auth
        localStorage.removeItem('access_token');
        break;
      default:
        console.error('OAuth error:', errorData.error_description);
    }

    throw new Error(errorData.error_description);
  }
}
```

#### Server-Side Error Handling

```javascript
// Express.js example
app.get('/oauth/callback', async (req, res) => {
  const { code, error, error_description } = req.query;

  if (error) {
    // Handle OAuth errors
    switch (error) {
      case 'access_denied':
        return res.render('oauth-denied');
      case 'invalid_request':
        return res.status(400).render('error', {
          message: 'Invalid OAuth request'
        });
      default:
        return res.status(500).render('error', {
          message: error_description || 'OAuth error occurred'
        });
    }
  }

  if (!code) {
    return res.status(400).render('error', {
      message: 'No authorization code received'
    });
  }

  try {
    // Exchange code for token
    const tokenResponse = await exchangeCodeForToken(code);
    // Handle success...
  } catch (error) {
    console.error('Token exchange failed:', error);
    res.status(500).render('error', {
      message: 'Failed to complete authorization'
    });
  }
});
```

## Security Considerations

### Client Security

#### Client Secret Protection
- **Never expose client secrets** in client-side code
- Store secrets securely on server-side only
- Use environment variables or secure key management
- Rotate secrets regularly

#### PKCE for Public Clients
- **Always use PKCE** for SPAs and mobile apps
- Generate cryptographically secure code verifiers
- Use SHA256 challenge method (`S256`)

#### State Parameter
- **Always include state parameter** for CSRF protection
- Generate cryptographically secure random values
- Validate state on callback

```javascript
// Good: Secure state generation
const state = crypto.randomUUID();

// Bad: Predictable state
const state = Date.now().toString();
```

### Server Security

#### Token Security
- Tokens are cryptographically secure (64 characters)
- Access tokens expire after 1 hour
- Authorization codes expire after 10 minutes
- Revoked tokens are immediately invalid

#### Database Security
- Client secrets are hashed using password-auth
- All tokens stored with expiration timestamps
- Automatic cleanup of expired codes/tokens
- Proper database indexes for performance

#### HTTPS Requirements
- **HTTPS required** for all production redirect URIs
- HTTP allowed only for localhost (development)
- Secure cookie settings for sessions

### Best Practices

#### For OAuth Clients

1. **Use PKCE** for all public clients
2. **Validate state parameter** to prevent CSRF
3. **Store tokens securely** (httpOnly cookies preferred)
4. **Handle token expiration** gracefully
5. **Use minimal scopes** (principle of least privilege)
6. **Implement proper error handling**

#### For Authorization Server

1. **Validate all parameters** strictly
2. **Use secure random generation** for all tokens
3. **Implement rate limiting** on endpoints
4. **Log security events** for monitoring
5. **Regular security audits** of OAuth flows

#### Example Secure Implementation

```javascript
class SecureOAuthClient {
  constructor(clientId, redirectUri) {
    this.clientId = clientId;
    this.redirectUri = redirectUri;
    this.baseUrl = 'https://api.newapp.example';
  }

  // Secure token storage
  storeToken(token, expiresIn) {
    const expiresAt = Date.now() + (expiresIn * 1000);

    // Use httpOnly cookie if possible, otherwise localStorage with expiration
    if (this.canUseHttpOnlyCookies()) {
      this.setSecureCookie('access_token', token, expiresIn);
    } else {
      localStorage.setItem('access_token', JSON.stringify({
        token,
        expiresAt
      }));
    }
  }

  getToken() {
    if (this.canUseHttpOnlyCookies()) {
      return this.getSecureCookie('access_token');
    } else {
      const stored = localStorage.getItem('access_token');
      if (!stored) return null;

      const { token, expiresAt } = JSON.parse(stored);
      if (Date.now() >= expiresAt) {
        localStorage.removeItem('access_token');
        return null;
      }

      return token;
    }
  }

  // Secure state validation
  async authorize(scopes = ['profile']) {
    const state = crypto.randomUUID();
    const { codeVerifier, codeChallenge } = await this.generatePKCE();

    // Store state and verifier securely
    sessionStorage.setItem('oauth_state', state);
    sessionStorage.setItem('oauth_code_verifier', codeVerifier);

    const authUrl = this.buildAuthUrl(scopes, state, codeChallenge);
    window.location.href = authUrl;
  }

  async handleCallback() {
    const urlParams = new URLSearchParams(window.location.search);
    const code = urlParams.get('code');
    const state = urlParams.get('state');
    const error = urlParams.get('error');

    // Validate state first
    const storedState = sessionStorage.getItem('oauth_state');
    if (!storedState || state !== storedState) {
      throw new Error('Invalid state parameter - possible CSRF attack');
    }

    if (error) {
      throw new Error(`OAuth error: ${error}`);
    }

    // Continue with token exchange...
  }
}
```

## Database Schema

### OAuth Tables Overview

The OAuth system uses three main tables with proper relationships and indexes:

```sql
-- OAuth clients (applications)
CREATE TABLE oauth_clients (
    id serial PRIMARY KEY,
    client_id varchar(255) UNIQUE NOT NULL,
    client_secret varchar(255) NOT NULL,  -- Hashed
    name varchar(255) NOT NULL,
    redirect_uris jsonb NOT NULL,         -- Array of allowed URIs
    scopes jsonb NOT NULL,                -- Array of allowed scopes
    active boolean DEFAULT true,
    created_at timestamptz DEFAULT NOW(),
    updated_at timestamptz DEFAULT NOW()
);

-- Authorization codes (temporary, 10 minute expiry)
CREATE TABLE oauth_authorization_codes (
    id serial PRIMARY KEY,
    code varchar(255) UNIQUE NOT NULL,
    client_id integer NOT NULL REFERENCES oauth_clients(id) ON DELETE CASCADE,
    user_id integer NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    redirect_uri text NOT NULL,
    scopes jsonb NOT NULL,                -- Granted scopes
    expires_at timestamptz NOT NULL,
    used boolean DEFAULT false,
    created_at timestamptz DEFAULT NOW()
);

-- Access tokens (1 hour expiry by default)
CREATE TABLE oauth_access_tokens (
    id serial PRIMARY KEY,
    token varchar(255) UNIQUE NOT NULL,
    client_id integer NOT NULL REFERENCES oauth_clients(id) ON DELETE CASCADE,
    user_id integer NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    scopes jsonb NOT NULL,                -- Granted scopes
    expires_at timestamptz NOT NULL,
    revoked boolean DEFAULT false,
    created_at timestamptz DEFAULT NOW()
);
```

### Performance Indexes

```sql
-- OAuth clients indexes
CREATE INDEX idx_oauth_clients_client_id ON oauth_clients(client_id);
CREATE INDEX idx_oauth_clients_active ON oauth_clients(active) WHERE active = true;
CREATE INDEX idx_oauth_clients_created_at ON oauth_clients(created_at);

-- Authorization codes indexes
CREATE INDEX idx_oauth_authorization_codes_code ON oauth_authorization_codes(code);
CREATE INDEX idx_oauth_authorization_codes_client_id ON oauth_authorization_codes(client_id);
CREATE INDEX idx_oauth_authorization_codes_user_id ON oauth_authorization_codes(user_id);
CREATE INDEX idx_oauth_authorization_codes_expires_at ON oauth_authorization_codes(expires_at);
CREATE INDEX idx_oauth_authorization_codes_used ON oauth_authorization_codes(used) WHERE used = false;

-- Access tokens indexes
CREATE INDEX idx_oauth_access_tokens_token ON oauth_access_tokens(token);
CREATE INDEX idx_oauth_access_tokens_client_id ON oauth_access_tokens(client_id);
CREATE INDEX idx_oauth_access_tokens_user_id ON oauth_access_tokens(user_id);
CREATE INDEX idx_oauth_access_tokens_expires_at ON oauth_access_tokens(expires_at);
CREATE INDEX idx_oauth_access_tokens_revoked ON oauth_access_tokens(revoked) WHERE revoked = false;
```

### Data Relationships

```
users (existing table)
  ↓ (1:many)
oauth_authorization_codes
  ↓ (1:1 exchange)
oauth_access_tokens

oauth_clients
  ↓ (1:many)
oauth_authorization_codes
  ↓ (1:many)
oauth_access_tokens
```

### Cleanup Operations

The system includes automatic cleanup functions:

```rust
// Clean up expired codes and tokens
let (codes_deleted, tokens_deleted) = db::cleanup_expired_oauth_data(&pool).await?;
```

This should be run periodically (e.g., daily cron job) to maintain database performance.

## Development Setup

### Prerequisites

- Rust 1.70+
- PostgreSQL 12+
- SQLx CLI for migrations

### Environment Variables

```bash
# Database connection
DATABASE_URL=postgresql://user:password@localhost/newapp

# OAuth settings (optional - uses defaults)
OAUTH_ACCESS_TOKEN_EXPIRY_HOURS=1
OAUTH_AUTH_CODE_EXPIRY_MINUTES=10
```

### Running Migrations

```bash
# Install SQLx CLI
cargo install sqlx-cli

# Run OAuth migrations
sqlx migrate run
```

### Development Server

```bash
# Start development server
cargo run

# OAuth endpoints will be available at:
# http://localhost:3000/oauth/authorize
# http://localhost:3000/oauth/token
# http://localhost:3000/oauth/userinfo
```

### Testing OAuth Flow

1. **Register a Test Client**

```sql
-- Insert test client directly into database
INSERT INTO oauth_clients (client_id, client_secret, name, redirect_uris, scopes, active)
VALUES (
    'test_client_123',
    '$argon2id$v=19$m=19456,t=2,p=1$VE0e0J0L0J0L0J0L0J0L0J$hash_here', -- Hash of 'test_secret'
    'Test Application',
    '["http://localhost:3000/callback"]',
    '["profile", "email"]',
    true
);
```

2. **Test Authorization Flow**

```bash
# 1. Open authorization URL in browser
open "http://localhost:3000/oauth/authorize?response_type=code&client_id=test_client_123&redirect_uri=http%3A//localhost%3A3000/callback&scope=profile%20email&state=test_state"

# 2. After user authorization, you'll be redirected to:
# http://localhost:3000/callback?code=AUTHORIZATION_CODE&state=test_state

# 3. Exchange code for token
curl -X POST http://localhost:3000/oauth/token \
  -H "Content-Type: application/x-www-form-urlencoded" \
  -d "grant_type=authorization_code" \
  -d "code=AUTHORIZATION_CODE_FROM_STEP_2" \
  -d "redirect_uri=http://localhost:3000/callback" \
  -d "client_id=test_client_123" \
  -d "client_secret=test_secret"

# 4. Use access token
curl -X GET http://localhost:3000/oauth/userinfo \
  -H "Authorization: Bearer ACCESS_TOKEN_FROM_STEP_3"
```

### Development Tools

#### Client Registration Helper

```rust
// Helper function for development
pub async fn create_development_client(pool: &PgPool) -> Result<(String, String), CustomError> {
    let client_id = utils::generate_client_id();
    let client_secret = utils::generate_client_secret();
    let client_secret_hash = utils::hash_client_secret(&client_secret)?;

    let redirect_uris = serde_json::json!([
        "http://localhost:3000/callback",
        "http://127.0.0.1:3000/callback"
    ]);
    let scopes = vec![OAuthScope::Profile, OAuthScope::Email];

    db::create_oauth_client(
        pool,
        &client_id,
        &client_secret_hash,
        "Development Client",
        &redirect_uris,
        &scopes,
    ).await?;

    println!("Created development client:");
    println!("Client ID: {}", client_id);
    println!("Client Secret: {}", client_secret);

    Ok((client_id, client_secret))
}
```

#### Token Inspection

```rust
// Debug helper to inspect tokens
pub async fn inspect_access_token(pool: &PgPool, token: &str) -> Result<(), CustomError> {
    match db::get_access_token_with_context(pool, token).await? {
        Some(token_info) => {
            println!("Token Info:");
            println!("  User: {} ({})", token_info.username, token_info.email);
            println!("  Client: {} ({})", token_info.client_name, token_info.client_identifier);
            println!("  Scopes: {:?}", token_info.parse_scopes()?);
            println!("  Expires: {}", token_info.expires_at);
            println!("  Revoked: {}", token_info.revoked);
        }
        None => println!("Token not found or expired"),
    }
    Ok(())
}
```

### Troubleshooting

#### Common Issues

**1. "Invalid client" error**
- Check client_id is correct
- Verify client is active in database
- Ensure client_secret is correct (if using)

**2. "Invalid redirect URI" error**
- Verify redirect_uri exactly matches registered URI
- Check URL encoding
- Ensure HTTPS for production domains

**3. "Invalid grant" error**
- Authorization code may be expired (10 minute limit)
- Code may have already been used
- Check redirect_uri matches authorization request

**4. "Access denied" error**
- User clicked "Deny" on authorization page
- This is normal user behavior, not a system error

**5. Token validation fails**
- Check Bearer token format: `Bearer ACCESS_TOKEN`
- Token may be expired (1 hour limit)
- Token may have been revoked

#### Debug Logging

Enable debug logging for OAuth operations:

```rust
// In main.rs or lib.rs
tracing_subscriber::fmt()
    .with_env_filter("newapp::api::oauth=debug")
    .init();
```

#### Database Queries for Debugging

```sql
-- Check client registration
SELECT * FROM oauth_clients WHERE client_id = 'your_client_id';

-- Check authorization codes
SELECT * FROM oauth_authorization_codes
WHERE client_id = (SELECT id FROM oauth_clients WHERE client_id = 'your_client_id')
ORDER BY created_at DESC;

-- Check access tokens
SELECT * FROM oauth_access_tokens
WHERE client_id = (SELECT id FROM oauth_clients WHERE client_id = 'your_client_id')
ORDER BY created_at DESC;

-- Clean up expired data
DELETE FROM oauth_authorization_codes WHERE expires_at < NOW();
DELETE FROM oauth_access_tokens WHERE expires_at < NOW();
```

## Production Deployment

### Security Checklist

- [ ] All redirect URIs use HTTPS
- [ ] Client secrets are properly hashed
- [ ] Rate limiting implemented on OAuth endpoints
- [ ] CORS configured appropriately
- [ ] Database connections are secure
- [ ] Logs don't contain sensitive data
- [ ] Regular cleanup of expired tokens scheduled
- [ ] Monitoring and alerting configured

### Performance Considerations

- **Database Indexes**: All critical queries are indexed
- **Connection Pooling**: Use appropriate pool sizes
- **Token Cleanup**: Schedule regular cleanup jobs
- **Caching**: Consider caching client information
- **Rate Limiting**: Implement per-client rate limits

### Monitoring

Monitor these key metrics:

- Authorization request rate
- Token exchange success rate
- Token validation errors
- Client authentication failures
- Database query performance
- Expired token cleanup frequency

### Backup and Recovery

- Regular database backups
- Client credential backup (encrypted)
- Disaster recovery procedures
- Token revocation procedures

## API Reference

### Data Types

#### OAuthScope

```rust
pub enum OAuthScope {
    ProfileRead,   // "profile:read" - Basic profile information
    ProfileEmail,  // "profile:email" - Email address access
    DataRead,     // "data:read" - API data read (customize)
    DataWrite,   // "data:write" - API data write (customize)
}
```

#### OAuthClient

```rust
pub struct OAuthClient {
    pub id: i32,
    pub client_id: String,
    pub client_secret: String,      // Hashed
    pub name: String,
    pub redirect_uris: serde_json::Value,  // JSON array
    pub scopes: serde_json::Value,         // JSON array
    pub active: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}
```

#### TokenResponse

```rust
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,         // Always "Bearer"
    pub expires_in: i64,           // Seconds until expiration
    pub scope: String,             // Space-separated scopes
}
```

#### UserInfoResponse

```rust
pub struct UserInfoResponse {
    pub sub: String,                    // User ID
    pub preferred_username: Option<String>,
    pub name: Option<String>,           // Display name
    pub email: Option<String>,          // If email scope granted
    pub email_verified: Option<bool>,   // If email scope granted
}
```

### Utility Functions

#### Token Generation

```rust
pub fn generate_client_id() -> String;          // 24 characters
pub fn generate_client_secret() -> String;     // 48 characters
pub fn generate_authorization_code() -> String; // 32 characters
pub fn generate_access_token() -> String;       // 64 characters
```

#### Security Functions

```rust
pub fn hash_client_secret(secret: &str) -> Result<String, OAuthError>;
pub fn verify_client_secret(secret: &str, hash: &str) -> bool;
```

#### Validation Functions

```rust
pub fn validate_redirect_uri(redirect_uri: &str) -> bool;
pub fn is_redirect_uri_allowed(redirect_uri: &str, allowed_uris: &serde_json::Value) -> bool;
pub fn validate_scopes_for_client(requested_scopes: &[OAuthScope], client_scopes: &[OAuthScope]) -> bool;
```

#### Time Functions

```rust
pub fn authorization_code_expiration() -> OffsetDateTime;  // +10 minutes
pub fn access_token_expiration() -> OffsetDateTime;        // +1 hour
pub fn is_expired(expires_at: OffsetDateTime) -> bool;
pub fn seconds_until_expiration(expires_at: OffsetDateTime) -> i64;
```

## Contributing

### Code Style

- Follow Rust standard formatting (`cargo fmt`)
- Use meaningful variable names
- Add comprehensive documentation
- Include unit tests for new functions

### Testing

```bash
# Run all tests
cargo test

# Run OAuth-specific tests
cargo test oauth

# Run with coverage
cargo tarpaulin --out Html
```

### Pull Request Guidelines

1. Include tests for new functionality
2. Update documentation as needed
3. Follow security best practices
4. Test OAuth flows manually
5. Check for breaking changes

## License

This OAuth implementation is part of this project and follows the same license terms.

## Support

For issues and questions:

1. Check this documentation first
2. Review the troubleshooting section
3. Check existing GitHub issues
4. Create a new issue with detailed information

---

*This documentation covers the complete OAuth 2.0 provider implementation in this repository. For the latest updates and additional examples, see the source code and tests.*
