# Official Gmail MCP preview setup

The selected route is Google's official preview. No preview application,
registration, account authorization or mail operation has been performed.
Checked against public documentation and unauthenticated metadata on 2026-10-22.

## Enrollment and project preparation

Confirm preview membership and the Google Cloud project first. Enrollment details
and terms require the user's review; this guide does not submit an application.
In the selected project, enable `gmail.googleapis.com` and
`gmailmcp.googleapis.com`. Configure OAuth branding, audience and test users.
Create a separate OAuth client marked **Use this client for an AI-powered agent**.
The official server is `https://gmailmcp.googleapis.com/mcp/v1` over Streamable HTTP.
See [Google's Gmail MCP setup guide](https://developers.google.com/workspace/gmail/api/guides/configure-mcp-server).

## Client and minimum permissions

A Desktop public client with S256 PKCE and a loopback redirect matches alloyOS's
current browser-assistance architecture. Google documents this general native
OAuth flow; Gmail preview acceptance with this client remains unverified. Do not
copy another product's hosted redirect URI. The helper's callback path is
`/oauth/callback`; use the registered port policy in the private native form.
See [Google's Desktop OAuth guide](https://developers.google.com/identity/protocols/oauth2/native-app).

For initial read/search acceptance, privately approve only
`https://www.googleapis.com/auth/gmail.readonly`. Google's broader Gmail MCP guide
also includes `gmail.compose`; that permission manages drafts and can send mail,
so it is outside this read/search test. See
[Gmail scope definitions](https://developers.google.com/workspace/gmail/api/auth/scopes).

## Current compatibility blockers

The live protected-resource document advertises
`authorization_servers: ["https://accounts.google.com/"]`, while the corresponding
authorization-server document declares `issuer: "https://accounts.google.com"`.
The exact issuer check currently rejects this mismatch. Do not normalize it away
or change the saved issuer to bypass discovery. These were public metadata GETs,
not an authenticated compatibility test:
[resource metadata](https://gmailmcp.googleapis.com/.well-known/oauth-protected-resource/mcp/v1),
[authorization-server metadata](https://accounts.google.com/.well-known/oauth-authorization-server).

Google requires `access_type=offline` for refresh-token requests; the
`offline_access` scope alone is insufficient. For an already granted account,
`prompt=consent` can request fresh consent. Neither parameter is currently
configurable in alloyOS; a typed, privately reviewed profile option is the next
implementation step. No arbitrary authorization-query overrides should be added.

## Acceptance after prerequisites are resolved

Review installation of the exact endpoint. Enter registration details only in
the native private client form, never in chat or ordinary files. Saving a profile
does not authorize or connect an account. Explicitly authorize, verify live
`tools/list`, then request the user's chosen read/search operation. Separately
verify reboot recovery, refresh and disconnect. Credentials stay OS-owned;
private storage currently provides access isolation, not encryption at rest.
