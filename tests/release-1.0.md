# 1.0 release verification

Release-ready: no

Do not tag or publish while required entries remain pending. This is a record
of evidence, not a promise that software can never fail.

| Check | Current evidence |
| --- | --- |
| Gateway protocol and CLI tests | 30 passed locally, including TCP/UDP/DNS/DHCP, two independent PSP clients and disconnect queue regressions |
| Street native game infrastructure chooser | Sony dialog result 0; IP 10.77.0.2, confirmed on hardware |
| Street normal XMB entry and icon | User confirmed both after a regular firmware return |
| Menu layout refusal/restoration | PC test passed; 1202 corruption cases; independent SHA-1 and byte comparison |
| Linux x86_64 / ARM64 binaries | Static musl builds complete with locked dependencies |
| Windows x86_64 binary | Cross-build complete; fresh VM smoke test pending |
| macOS universal binary | Native CI build and Intel/Apple Silicon execution pending |
| Actual release binaries on five native runners | Workflow prepared; execution pending |
| Current 1.0 host beside PSPLink | Repeated hardware connections/transfers pending |
| XMB and PSPDX recording | Safe userspace capture verified on XMB; full sequence pending controller-helper approval |

The recording must show the real Street's original XMB network menu and its
successful connection, followed by PSPDX connecting and loading its catalog.
Record the actual gateway output concurrently; preserve real timing and label
separate Windows/Linux sessions correctly. In README the PSP is full width;
Windows and Linux CLI footage sits below in two smaller columns. Do not
substitute old 0.1.10 footage for this verification.

Hardware constraints and the outstanding controller-helper exception are in
AGENTS.md and tests/capture/README.md. No power cycling or reset loop is part
of the verification.
