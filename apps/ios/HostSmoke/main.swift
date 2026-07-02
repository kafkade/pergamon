// Host (macOS) smoke test for the pergamon UniFFI facade.
//
// Compiled and run by `scripts/smoke-macos.sh`. It links the macOS build of
// `pergamon-uniffi` and exercises the generated Swift bindings end-to-end
// (including the `Library` handle and the throwing `item(id:)` error path),
// which is a fast inner-loop check that the FFI contract works before the
// heavier iOS Simulator build. The file must be named `main.swift` so Swift
// permits the top-level executable code below.

import Foundation

print("pergamon-core via UniFFI → version \(libraryVersion())")

let library = Library()
let items = library.items()
print("Library().items(): \(items.count) items")
for it in items {
    let published = it.publishedAtMillis.map { String($0) } ?? "—"
    print("  • [\(it.status)] \(it.title)  type=\(it.contentType) read=\(it.readingMinutes)min pub=\(published)")
}

let archived = library.itemsWithStatus(status: .archived)
print("itemsWithStatus(.archived): \(archived.count)")

do {
    if let first = items.first {
        let opened = try library.item(id: first.id)
        print("item(\(first.id)) → opened \"\(opened.title)\" by \(opened.author ?? "unknown")")
    }
    // Exercise the ADR-019 error mapping: malformed ids throw PergamonError.
    _ = try library.item(id: "not-a-uuid")
    print("item(bogus) → unexpectedly succeeded")
} catch let error as PergamonError {
    print("item(bogus) → threw \(error)")
}

print("search(\"inoreader\") → \(library.search(query: "inoreader").count)")
print("readingMinutes(1000 words) → \(readingMinutes(text: String(repeating: "word ", count: 1000)))")
