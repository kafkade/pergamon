// Host (macOS) smoke test for the pergamon UniFFI facade.
//
// Compiled and run by `scripts/smoke-macos.sh`. It links the macOS build of
// `pergamon-uniffi` and exercises the generated Swift bindings end-to-end, which
// is a fast inner-loop check that the FFI contract works before the heavier iOS
// Simulator build. The file must be named `main.swift` so Swift permits the
// top-level executable code below.

import Foundation

print("pergamon-core via UniFFI → version \(libraryVersion())")

let items = sampleItems()
print("sampleItems(): \(items.count) items")
for it in items {
    let published = it.publishedAtMillis.map { String($0) } ?? "—"
    print("  • [\(it.status)] \(it.title)  type=\(it.contentType) read=\(it.readingMinutes)min pub=\(published)")
}

let archived = itemsWithStatus(status: .archived)
print("itemsWithStatus(.archived): \(archived.count)")

if let first = items.first, let opened = getItem(id: first.id) {
    print("getItem(\(first.id)) → opened \"\(opened.title)\" by \(opened.author ?? "unknown")")
}
print("getItem(bogus) → \(String(describing: getItem(id: "not-a-uuid")))")
print("readingMinutes(1000 words) → \(readingMinutes(text: String(repeating: "word ", count: 1000)))")
