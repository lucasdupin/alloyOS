# Optional QEMU browser assistance

alloyOS owns OAuth state, PKCE, provider metadata validation, token exchange,
refresh, revocation and private credential records. The QEMU development host
helper only prepares a loopback listener, opens a requested authorization URL
with the platform browser, returns a bounded callback query once, and cancels
that listener. BROWSER1 frames are private transport, never chat messages.
The OS validates callback state and issuer; a delivered callback is not proof of
authorized access. Missing browser support is an explicit error.

The helper has no client profiles, credential vault or Keychain integration.
macOS uses `open`; Linux uses `xdg-open`. Applications require neither launcher:
these are optional development-host facilities, not app runtimes. Native hardware
will need its own browser/callback arrangement; this helper is not a hardware
port. A provider and native-client registration policy still need to be chosen
before real account use.

Trusted OS provisioning binds a private client profile to an exact MCP resource.
The kernel store uses `/system/accounts`, bypassing public VFS caches through a
private adapter. Model tools, scripts and app filesystem access cannot read,
write or list that subtree. Public account status contains no tokens, codes,
client secrets or PKCE verifier. CobaltFS records are **unencrypted at rest**;
this does not provide protection against offline disk access or claim hardware
power-loss/fsync guarantees. A RAM filesystem explicitly reports volatile storage.

A durable revocation marker blocks a previous credential before its record is
removed. Interrupted or corrupt current state fails closed; no older checkpoint
is recovered. Failed storage operations remain errors. Deletion does not promise
raw block erasure or successful provider-side revocation.

The optional QEMU HTTPS relay can transiently see authorization headers and token
exchange plaintext while carrying guest requests. OS credential ownership does
not make that development relay a confidential boundary against the host.
Hermetic browser, OAuth and CobaltFS tests use synthetic credentials and providers;
they do not establish real Gmail interoperability or hardware acceptance.
