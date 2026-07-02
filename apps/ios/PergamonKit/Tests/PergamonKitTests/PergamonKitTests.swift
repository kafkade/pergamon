import XCTest
@testable import PergamonKit

/// Exercises the idiomatic wrapper against the Rust core through the generated
/// UniFFI bindings. Runs natively on the macOS host (`swift test`) via the
/// XCFramework's macOS slice — no Simulator required.
final class PergamonKitTests: XCTestCase {
    private var library: Library!

    override func setUp() {
        super.setUp()
        library = Library()
    }

    override func tearDown() {
        library = nil
        super.tearDown()
    }

    func testListsAllItems() {
        XCTAssertEqual(library.items().count, 5)
    }

    func testInboxReturnsOnlyInboxItems() {
        let inbox = library.inbox()
        XCTAssertEqual(inbox.count, 1)
        XCTAssertTrue(inbox.allSatisfy { $0.status == .inbox })
    }

    func testFiltersByStatus() {
        XCTAssertEqual(library.itemsWithStatus(status: .archived).count, 1)
        XCTAssertEqual(library.itemsWithStatus(status: .inbox).count, 1)
        XCTAssertTrue(library.itemsWithStatus(status: .discarded).isEmpty)
    }

    func testOpensKnownItem() throws {
        let first = try XCTUnwrap(library.items().first)
        let fetched = try library.item(id: first.id)
        XCTAssertEqual(fetched.title, first.title)
        XCTAssertEqual(fetched.id, first.id)
    }

    func testOpenThrowsNotFoundForUnknownId() {
        // A well-formed UUID that is not in the seeded corpus.
        let unknown = "00000000-0000-0000-0000-0000000003e7"
        XCTAssertThrowsError(try library.item(id: unknown)) { error in
            guard case PergamonError.NotFound = error else {
                return XCTFail("expected NotFound, got \(error)")
            }
        }
    }

    func testOpenThrowsInvalidInputForMalformedId() {
        XCTAssertThrowsError(try library.item(id: "not-a-uuid")) { error in
            guard case PergamonError.InvalidInput = error else {
                return XCTFail("expected InvalidInput, got \(error)")
            }
        }
    }

    func testSearchMatchesCaseInsensitivelyAcrossFields() {
        XCTAssertEqual(library.search(query: "inoreader").count, 1)
        XCTAssertEqual(library.search(query: "RESEARCHER").count, 1)
        XCTAssertTrue(library.search(query: "   ").isEmpty)
        XCTAssertTrue(library.search(query: "no-such-content").isEmpty)
    }

    func testReadingMinutesHelper() {
        XCTAssertEqual(readingMinutes(text: ""), 0)
        XCTAssertGreaterThanOrEqual(
            readingMinutes(text: String(repeating: "word ", count: 238)),
            1
        )
    }

    func testLibraryVersionIsNonEmpty() {
        XCTAssertFalse(libraryVersion().isEmpty)
    }

    func testPublishedDateMapsFromEpochMillis() throws {
        let item = try library.item(id: "00000000-0000-0000-0000-000000000001")
        let date = try XCTUnwrap(item.publishedDate)
        // 1_577_836_800 s == 2020-01-01T00:00:00Z.
        XCTAssertEqual(date.timeIntervalSince1970, 1_577_836_800, accuracy: 0.001)
    }

    func testConvenienceLabels() {
        XCTAssertEqual(Status.archived.label, "Archived")
        XCTAssertEqual(ContentType.pdf.label, "PDF")
    }

    func testExposesContentTextAndSource() throws {
        let item = try library.item(id: "00000000-0000-0000-0000-000000000001")
        XCTAssertFalse(item.contentText?.isEmpty ?? true)
        XCTAssertEqual(item.sourceName, "Ink & Switch")
    }

    func testSourcesAreDistinctAndSorted() {
        XCTAssertEqual(
            library.sources(),
            ["Ink & Switch", "Memory Weekly", "Reader Diaries", "Rust Mobile Weekly"]
        )
    }

    func testSeededInboxItemStartsUnread() {
        XCTAssertTrue(library.inbox().allSatisfy { !$0.isRead })
    }

    func testMarkReadThenUnreadTogglesReadState() throws {
        let id = "00000000-0000-0000-0000-000000000001"

        let read = try library.markRead(id: id)
        XCTAssertTrue(read.isRead)
        XCTAssertNotNil(read.readDate)

        let unread = try library.markUnread(id: id)
        XCTAssertFalse(unread.isRead)
        XCTAssertNil(unread.readDate)
    }

    func testArchiveSetsStatusAndMarksRead() throws {
        let id = "00000000-0000-0000-0000-000000000001"
        let archived = try library.archive(id: id)
        XCTAssertEqual(archived.status, .archived)
        XCTAssertTrue(archived.isRead)
        XCTAssertEqual(library.itemsWithStatus(status: .archived).count, 2)
        XCTAssertTrue(library.inbox().isEmpty)
    }

    func testSaveForLaterMovesItemToLater() throws {
        let id = "00000000-0000-0000-0000-000000000001"
        let saved = try library.saveForLater(id: id)
        XCTAssertEqual(saved.status, .later)
        XCTAssertEqual(library.itemsWithStatus(status: .later).count, 2)
    }

    func testMutationsThrowForMalformedAndUnknownIds() {
        XCTAssertThrowsError(try library.markRead(id: "not-a-uuid")) { error in
            guard case PergamonError.InvalidInput = error else {
                return XCTFail("expected InvalidInput, got \(error)")
            }
        }
        XCTAssertThrowsError(
            try library.archive(id: "00000000-0000-0000-0000-0000000003e7")
        ) { error in
            guard case PergamonError.NotFound = error else {
                return XCTFail("expected NotFound, got \(error)")
            }
        }
    }

    // MARK: - Organization: tags & collections

    private func uuid(_ n: UInt64) -> String {
        String(format: "00000000-0000-0000-0000-%012x", n)
    }

    func testSeededItemExposesTagsAndCollections() throws {
        let item = try library.item(id: uuid(1))
        XCTAssertEqual(item.tags, ["local-first", "reading"])
        XCTAssertEqual(item.collectionIds, [uuid(101)])
    }

    func testTagsAreSortedWithCounts() {
        let tags = library.tags()
        XCTAssertEqual(tags.map(\.name), ["ios", "local-first", "memory", "reading", "rust"])
        XCTAssertEqual(tags.first { $0.name == "reading" }?.itemCount, 3)
    }

    func testCollectionsAreTreeOrderedWithDepth() {
        let collections = library.collections()
        XCTAssertEqual(
            collections.map { [$0.name, "\($0.depth)", "\($0.itemCount)"] },
            [
                ["Reading List", "0", "2"],
                ["Deep Dives", "1", "1"],
                ["Tech", "0", "1"],
            ]
        )
    }

    func testItemsWithTagIsCaseInsensitive() {
        XCTAssertEqual(library.itemsWithTag(tag: "READING").count, 3)
        XCTAssertTrue(library.itemsWithTag(tag: "   ").isEmpty)
    }

    func testItemsInCollectionListsMembersAndThrowsForUnknown() throws {
        XCTAssertEqual(try library.itemsInCollection(collectionId: uuid(101)).count, 2)
        XCTAssertThrowsError(try library.itemsInCollection(collectionId: uuid(999))) { error in
            guard case PergamonError.NotFound = error else {
                return XCTFail("expected NotFound, got \(error)")
            }
        }
    }

    func testAddTagCreatesAndIsIdempotent() throws {
        let tagged = try library.addTag(id: uuid(4), name: "Focus")
        XCTAssertEqual(tagged.tags, ["Focus"])
        // Case-insensitive idempotency: no duplicate.
        let again = try library.addTag(id: uuid(4), name: "focus")
        XCTAssertEqual(again.tags, ["Focus"])
    }

    func testAddTagRejectsBlank() {
        XCTAssertThrowsError(try library.addTag(id: uuid(1), name: "  ")) { error in
            guard case PergamonError.InvalidInput = error else {
                return XCTFail("expected InvalidInput, got \(error)")
            }
        }
    }

    func testRemoveTagIsIdempotent() throws {
        let removed = try library.removeTag(id: uuid(1), name: "READING")
        XCTAssertEqual(removed.tags, ["local-first"])
        let again = try library.removeTag(id: uuid(1), name: "reading")
        XCTAssertEqual(again.tags, ["local-first"])
    }

    func testCreateCollectionNestsAndValidates() throws {
        let created = try library.createCollection(name: "Later Reads", parentId: uuid(101))
        XCTAssertEqual(created.parentId, uuid(101))
        XCTAssertEqual(created.depth, 1)
        XCTAssertThrowsError(try library.createCollection(name: "  ", parentId: nil)) { error in
            guard case PergamonError.InvalidInput = error else {
                return XCTFail("expected InvalidInput, got \(error)")
            }
        }
    }

    func testAddAndRemoveFromCollectionAreIdempotent() throws {
        let added = try library.addToCollection(id: uuid(4), collectionId: uuid(103))
        XCTAssertEqual(added.collectionIds, [uuid(103)])
        let again = try library.addToCollection(id: uuid(4), collectionId: uuid(103))
        XCTAssertEqual(again.collectionIds, [uuid(103)])
        let removed = try library.removeFromCollection(id: uuid(4), collectionId: uuid(103))
        XCTAssertTrue(removed.collectionIds.isEmpty)
    }

    func testSearchFilteredCombinesFacets() {
        // Status facet.
        let later = SearchFacets(
            contentType: nil, status: .later, tag: nil, source: nil,
            sinceMillis: nil, beforeMillis: nil
        )
        XCTAssertEqual(library.searchFiltered(query: "", facets: later).count, 1)

        // Tag facet with a text query (AND-combined).
        let rust = SearchFacets(
            contentType: nil, status: nil, tag: "rust", source: nil,
            sinceMillis: nil, beforeMillis: nil
        )
        let hits = library.searchFiltered(query: "word", facets: rust)
        XCTAssertEqual(hits.count, 1)
        XCTAssertTrue(hits[0].tags.contains("rust"))

        // Empty query with no facets matches nothing.
        XCTAssertTrue(
            library.searchFiltered(
                query: "",
                facets: SearchFacets(
                    contentType: nil, status: nil, tag: nil, source: nil,
                    sinceMillis: nil, beforeMillis: nil
                )
            ).isEmpty
        )
    }
}

