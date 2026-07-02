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
}
