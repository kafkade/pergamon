# ADR-008: Licensing — Apache-2.0 + AGPL-3.0

**Status**: Accepted  
**Date**: 2026-05-22  
**Deciders**: kafkade

## Context

pergamon has two different distribution models inside one project family. The client-side software—CLI, TUI, storage, import/export, core logic, Obsidian integration, and future mobile bindings—is intended to be broadly reusable, embeddable, and contributor-friendly. The sync server, however, creates a different risk: a third party could offer pergamon as a hosted service without contributing improvements back to the project. Because pergamon is being built as open source by a solo developer, the licensing strategy must protect both collaboration and sustainability.

MIT would be permissive enough for the client side, but Apache-2.0 offers a stronger patent grant and is already consistent with other kafkade projects such as ldgr and tock. That consistency matters for contributor expectations and project hygiene. On the server side, a normal copyleft license would not fully address the hosted-service case. AGPL-3.0 is designed specifically to require source availability when software is offered over a network.

The architecture also includes strict crate boundaries. Mixing server-only code into client crates would create confusion about which license applies and increase the risk of accidental contamination or weakened enforcement. The licensing decision therefore depends on clean separation as much as on license texts themselves.

## Decision

pergamon will adopt a split licensing model:

- Apache-2.0 for all client-side crates and applications, including `pergamon-core`, `pergamon-cli`, `pergamon-storage`, `pergamon-feed`, `pergamon-extract`, `pergamon-import`, `pergamon-export`, and the Obsidian plugin
- AGPL-3.0 for `pergamon-server`, the Axum-based sync server

Apache-2.0 is chosen over MIT because it includes an explicit patent grant and aligns with the kafkade project family. AGPL-3.0 is chosen for the server to discourage proprietary hosted pergamon sync offerings that do not contribute changes back.

A structural rule accompanies the licensing decision: server code must not be moved into client crates, and client code must not be moved into server-only crates in ways that blur the licensing boundary. Shared logic belongs in Apache-licensed client/shared crates; server-specific sync and network service code belongs in the AGPL server.

## Consequences

### Positive

- Encourages broad reuse of client libraries and tools.
- Provides a patent grant and consistency with related kafkade projects.
- Protects the sync server from closed hosted-service appropriation.
- Keeps licensing understandable when crate boundaries are respected.
- Supports future community contributions without forcing all clients under copyleft.

### Negative

- Dual licensing across a monorepo requires clear documentation and discipline.
- Some potential adopters may avoid AGPL components entirely.
- Contributors must understand which crate falls under which license.
- Accidental code movement across boundaries could create legal and maintenance issues.

## Rejected Alternatives

- **MIT for everything**: rejected because it lacks Apache’s patent protections and does not align as well with existing kafkade projects.
- **Apache-2.0 for everything, including the server**: rejected because it would permit proprietary hosted sync offerings without reciprocity.
- **AGPL-3.0 for the entire project**: rejected because it would unnecessarily restrict client reuse and integrations.
- **Single-license monorepo without boundary rules**: rejected because pergamon’s client/server goals differ and need explicit separation.
