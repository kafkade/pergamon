// Re-export the UniFFI-generated bindings so downstream consumers import a single
// module (`PergamonKit`) and never reference the generated `PergamonBindings`
// target — or the raw FFI symbols — directly. This is the "no hand-written FFI
// glue" boundary from ADR-019: the app sees `Library`, `ContentItem`,
// `ContentType`, `Status`, and `PergamonError` through PergamonKit.
@_exported import PergamonBindings
