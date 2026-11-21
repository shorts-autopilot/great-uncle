# Security

## Reporting a problem

Report suspected vulnerabilities privately to the maintainers rather than in a
public issue. Include the version (`openlight --version`), the platform, the
input that triggers it and, for a parser problem, the smallest file that
reproduces it. Please do not attach a real venue's show file; reduce it first.

## What the project protects

- **Untrusted archives and documents.** `.gdtf`, `.mvr` and `.show` files are
  read from outside the project. Entry counts, uncompressed sizes, paths and
  show-file size are checked before parsing. The limits and their tests live in
  `crates/engine/src/limits.rs`.
- **The remote is opt-in and loopback by default.** OSC has no authentication;
  the HTTP page uses a bearer token stored in `~/.openlight/remote.token`.
  `--remote-bind` is an explicit opt-in. There is no TLS: run the remote on a
  trusted show network, or use `--no-remote`.
- **The desk lock is not authentication.** `Lock <code>` stops accidental
  changes from the command line. The code is plaintext in the desk file, the
  file is not encrypted, and anyone who can edit the file or the binary can
  bypass it. It protects a desk from a wandering hand, not from an attacker.

## What is out of scope

- A malicious operator with filesystem access to the show or desk file.
- Network attackers on a hostile network where the remote was deliberately
  exposed. Plain HTTP and unauthenticated OSC are documented limitations.
- The physical DMX bus and devices connected to it.
