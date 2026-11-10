# Third-party components

This source repository is MIT licensed; dependencies retain their original licenses.

- GPUI Kit / gpui-kit-assets 0.9.1: Apache-2.0, https://github.com/longbridge/gpui-kit and published crate licensing.
- sqldef/sqlite3def: MIT for main code, Apache-2.0 for its parser. Installed separately via the sqldef Homebrew tap, not redistributed inside this executable. https://github.com/sqldef/sqldef/blob/master/LICENSE
- NVIDIA Parakeet TDT 0.6B v3 model: CC-BY-4.0, https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3 (NVIDIA). The optional Q8_0 GGUF is a converted/quantized model, downloaded separately and not bundled in the executable.
- Sonora 0.3.0 and its audio-processing crates: BSD-3-Clause, https://github.com/dignifiedquire/sonora. Pure Rust WebRTC AEC3 processing; no additional runtime service.
- GCF Rust encoder/decoder (`gcf` 3.0.1): MIT, copyright 2026 Dayna Blackwell. https://crates.io/crates/gcf/3.0.1 (the pinned crate includes its MIT LICENSE).
- SQLite: public domain, https://sqlite.org/copyright.html
- agent-desktop: Apache-2.0; development/test dependency only, https://github.com/vercel-labs/agent-desktop

Cargo.lock files identify the resolved Rust dependencies. This list is an overview, not a complete binary redistribution license inventory. Any future bundled release must include all notices required by its resolved dependencies. Speech models and the nemo-speech runtime are not included in the Homebrew package.
