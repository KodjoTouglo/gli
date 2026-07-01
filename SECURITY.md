# Security Policy

## Reporting a vulnerability

Please report security issues privately, not as a public issue or pull request.

Open a private advisory at
https://github.com/KodjoTouglo/hardn/security/advisories/new, or email the
maintainer if you cannot use advisories.

Include a description, the affected version, and steps to reproduce. You will
get an acknowledgement, and we will work on a fix and coordinate disclosure.

## Scope

gli changes SSH, firewall, users, and other sensitive system state. Issues
of particular interest:

- Lockout or rollback failures that could leave a host unreachable.
- Command or shell injection through config values or remote output.
- Host key or authentication handling in the remote (SSH) path.
- Weak defaults in a security module.

## Supported versions

The project is pre-1.0; only the latest release and `develop` receive fixes.
