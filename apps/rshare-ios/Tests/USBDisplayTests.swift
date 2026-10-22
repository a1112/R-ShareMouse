import XCTest
@testable import RShareMouse

final class USBDisplayTests: XCTestCase {
    func testLANKeyRequires256RandomBitsInsteadOfASixDigitCode() {
        XCTAssertEqual(DisplayLAN.key(String(repeating: "ab", count: 32)), Data(repeating: 0xab, count: 32))
        for invalid in ["123456", String(repeating: "g", count: 64), String(repeating: "a", count: 63)] {
            XCTAssertNil(DisplayLAN.key(invalid))
        }
    }
    func testUSBFramingHandlesFragmentationAndCoalescing() throws {
        var parser = USBPacketParser()
        let frame = USBPacketParser.packet(kind: 2, body: Data([0, 255, 4]))
        XCTAssertTrue(try parser.append(frame.prefix(3)).isEmpty)
        let result = try parser.append(frame.dropFirst(3) + USBPacketParser.packet(kind: 4, body: Data()))
        XCTAssertEqual(result.map(\.kind), [2, 4])
        XCTAssertEqual(result[0].body, Data([0, 255, 4]))
    }
    func testUSBFramingRejectsOversizedAndUnknownMessages() {
        for bytes: [UInt8] in [[0,128,0,1,2], [0,0,0,0,255]] {
            var parser = USBPacketParser()
            XCTAssertThrowsError(try parser.append(Data(bytes)))
        }
    }
    func testVideoPacketRejectsTruncatedParameterSetsAndNALUnits() {
        XCTAssertThrowsError(try USBVideoPacket(Data(repeating: 0, count: 10)))
        var bytes = Data(repeating: 0, count: 12); bytes[9] = 8
        XCTAssertThrowsError(try USBVideoPacket(bytes))
        XCTAssertThrowsError(try USBVideoPacket(Data(repeating: 0, count: 16)))
    }
}
