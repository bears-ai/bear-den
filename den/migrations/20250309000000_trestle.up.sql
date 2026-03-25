-- Trestle minimal schema: users, invites, email, OAuth provider

SET statement_timeout = 0;
SET lock_timeout = 0;

CREATE TABLE users (
    id serial PRIMARY KEY,
    email character varying NOT NULL,
    created_at timestamp without time zone DEFAULT now() NOT NULL,
    updated_at timestamp without time zone DEFAULT now() NOT NULL,
    display_name character varying(255) DEFAULT ''::character varying NOT NULL,
    username character varying(30) DEFAULT ''::character varying NOT NULL UNIQUE,
    passhash text DEFAULT ''::text NOT NULL,
    admin_flag boolean DEFAULT false,
    theme text NOT NULL DEFAULT 'system',
    week_start_day integer NOT NULL DEFAULT 1,
    premium_until timestamp without time zone DEFAULT NULL,
    CONSTRAINT username_alphanumeric CHECK (username::text ~ '^[a-zA-Z0-9]+$'::text)
);

CREATE TABLE invites (
    id serial PRIMARY KEY,
    code text NOT NULL UNIQUE,
    user_id integer NOT NULL REFERENCES users (id),
    new_user_id integer REFERENCES users (id),
    created_at timestamp without time zone DEFAULT now(),
    updated_at timestamp without time zone DEFAULT now()
);

CREATE TABLE email_configs (
    id serial PRIMARY KEY,
    user_id integer NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    email_address text NOT NULL,
    verify_code text DEFAULT NULL,
    verify_code_expire_at timestamp with time zone NOT NULL DEFAULT (now() + '1 hour'::interval),
    verified_at timestamp with time zone NULL,
    active boolean NOT NULL DEFAULT false,
    current_error_count integer DEFAULT 0,
    last_error jsonb,
    created_at timestamp without time zone DEFAULT now(),
    updated_at timestamp without time zone DEFAULT now()
);

CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE email_messages (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    email_config_id integer NOT NULL REFERENCES email_configs (id),
    message_id text NULL,
    message_type text NOT NULL,
    parameters jsonb NULL,
    sent_at timestamptz NULL,
    failed_at timestamptz NULL,
    response_message text NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE oauth_clients (
    id serial PRIMARY KEY,
    client_id varchar(255) UNIQUE NOT NULL,
    client_secret varchar(255) NULL,
    name varchar(255) NOT NULL,
    redirect_uris jsonb NOT NULL,
    scopes jsonb NOT NULL,
    active boolean DEFAULT true,
    trusted boolean DEFAULT false,
    public boolean DEFAULT false,
    created_at timestamptz DEFAULT now(),
    updated_at timestamptz DEFAULT now()
);

CREATE INDEX idx_oauth_clients_client_id ON oauth_clients (client_id);
CREATE INDEX idx_oauth_clients_active ON oauth_clients (active) WHERE active = true;
CREATE INDEX idx_oauth_clients_trusted ON oauth_clients (trusted) WHERE trusted = true;
CREATE INDEX idx_oauth_clients_public ON oauth_clients (public) WHERE public = true;

CREATE TABLE oauth_authorization_codes (
    id serial PRIMARY KEY,
    code varchar(255) UNIQUE NOT NULL,
    client_id integer NOT NULL REFERENCES oauth_clients (id) ON DELETE CASCADE,
    user_id integer NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    redirect_uri text NOT NULL,
    scopes jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    used boolean DEFAULT false,
    code_challenge varchar(255),
    code_challenge_method varchar(10),
    created_at timestamptz DEFAULT now()
);

CREATE INDEX idx_oauth_authorization_codes_code ON oauth_authorization_codes (code);
CREATE INDEX idx_oauth_authorization_codes_client_id ON oauth_authorization_codes (client_id);
CREATE INDEX idx_oauth_authorization_codes_user_id ON oauth_authorization_codes (user_id);

CREATE TABLE oauth_access_tokens (
    id serial PRIMARY KEY,
    token text UNIQUE NOT NULL,
    client_id integer NOT NULL REFERENCES oauth_clients (id) ON DELETE CASCADE,
    user_id integer NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    scopes jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    revoked boolean DEFAULT false,
    created_at timestamptz DEFAULT now()
);

CREATE INDEX idx_oauth_access_tokens_token ON oauth_access_tokens (token);
CREATE INDEX idx_oauth_access_tokens_client_id ON oauth_access_tokens (client_id);
CREATE INDEX idx_oauth_access_tokens_user_id ON oauth_access_tokens (user_id);

CREATE TABLE oauth_refresh_tokens (
    id serial PRIMARY KEY,
    token varchar(255) UNIQUE NOT NULL,
    client_id integer NOT NULL REFERENCES oauth_clients (id) ON DELETE CASCADE,
    user_id integer NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    scopes jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    revoked boolean DEFAULT false,
    created_at timestamptz DEFAULT now()
);

CREATE INDEX idx_oauth_refresh_tokens_token ON oauth_refresh_tokens (token);
