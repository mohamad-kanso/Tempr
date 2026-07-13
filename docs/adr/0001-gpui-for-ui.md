# ADR-0001: GPUI for UI

- **Status:** Accepted
- **Date:** 2026-07-13
- **Deciders:** Project brief (founding decision)

## Context

Tempr is a native Rust IDE that requires a UI framework capable of GPU-accelerated rendering, complex layouts, and custom theming. The framework must align with the performance pillar — no web views, no Electron, no garbage-collected runtimes. It must support the rendering of hundreds of thousands of rows in virtualized lists, compositing of custom-drawn components, and a deep theming system that gives users full control over every pixel.

## Decision

Use GPUI (from the Zed project) as the sole UI framework for all of Tempr's interface.

## Alternatives Considered

**egui** — An immediate-mode GUI library for Rust. While ergonomic for simple tools, egui's immediate-mode paradigm imposes limits on complex layout composition and makes deep, user-customizable theming difficult to implement. Virtualized list performance is not comparable to retained-mode GPU pipelines. Rejected due to insufficient theming depth and layout flexibility.

**Qt** — A mature, cross-platform UI toolkit. Qt introduces a C++ FFI surface that complicates Rust integration and requires navigating complex build systems (moc, uic). Licensing concerns around Qt's commercial terms also pose risks for an open-source project. The FFI boundary creates maintenance burden and potential unsoundness. Rejected due to FFI complexity and licensing.

**Electron / Tauri** — Web-technology-based shells that embed a browser runtime. Both violate the performance pillar by introducing significant memory footprint overhead and cold-start latency. Electron bundles Chromium; Tauri uses the system WebView but still carries web rendering costs. Neither achieves the sub-100ms startup and low memory targets. Rejected due to performance characteristics.

**iced** — A Rust-native Elm-architecture GUI library. At the time of evaluation, iced's virtualized list widget was not mature enough to handle the scale of results and tree views Tempr requires. The ecosystem around custom widget development was less battle-tested compared to GPUI. Rejected due to maturity of virtualized list components.

## Consequences

- GPUI is a bleeding-edge dependency pinned to Zed's development cadence; API changes can be breaking and rapid.
- A custom component library is required beyond what GPUI provides out of the box (see `11-gpui.md`).
- Direct access to Zed's rendering innovations — GPU shaders, efficient text layout, compositing.
- Risk of API instability is mitigated by pinning a specific GPUI revision and wrapping all GPUI usage in a thin component layer.

## Related

- [02-architecture.md](../02-architecture.md)
- [11-gpui.md](../11-gpui.md)
- [ADR-0002](0002-rust-only.md)
