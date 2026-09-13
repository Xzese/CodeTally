#!/usr/bin/env swift
import AppKit
import Foundation
import Vision

// Run from the repository root. Raw captures never enter the repository.
// OCR is an aid, not proof of anonymity: visually review every output before committing.
struct CaptureError: Error, CustomStringConvertible { let description: String }
func fail(_ message: String) throws -> Never { throw CaptureError(description: message) }
func run(_ executable: String, _ arguments: [String]) throws -> Data {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments
    let output = Pipe()
    let errors = Pipe()
    process.standardOutput = output
    process.standardError = errors
    try process.run()
    let data = output.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    guard process.terminationStatus == 0 else {
        // Child output may contain identifying data; do not print it.
        try fail("\(URL(fileURLWithPath: executable).lastPathComponent) failed (\(process.terminationStatus)). Check permissions and arguments.")
    }
    return data
}
func rectangle(_ value: String) throws -> CGRect {
    let parts = value.split(separator: ",").compactMap { Double($0) }
    guard parts.count == 4, parts.allSatisfy({ $0.isFinite }), parts[2] > 0, parts[3] > 0 else {
        try fail("Rectangles must be x,y,width,height with positive dimensions.")
    }
    return CGRect(x: parts[0], y: parts[1], width: parts[2], height: parts[3])
}
func observations(_ image: CGImage) throws -> [VNRecognizedTextObservation] {
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = false
    try VNImageRequestHandler(cgImage: image).perform([request])
    return request.results ?? []
}
func patterns(for identifiers: [String]) throws -> [NSRegularExpression] {
    try identifiers.map { value in
        let words = value.components(separatedBy: CharacterSet.alphanumerics.inverted).filter { !$0.isEmpty }
        let pattern = words.map(NSRegularExpression.escapedPattern(for:)).joined(separator: "[^a-zA-Z0-9]*")
        return try NSRegularExpression(pattern: pattern, options: [.caseInsensitive])
    }
}
func sensitiveRegions(_ image: CGImage, _ patterns: [NSRegularExpression]) throws -> [CGRect] {
    var regions: [CGRect] = []
    for observation in try observations(image) {
        guard let candidate = observation.topCandidates(1).first else { continue }
        let text = candidate.string
        for pattern in patterns {
            for match in pattern.matches(in: text, range: NSRange(text.startIndex..., in: text)) {
                guard let range = Range(match.range, in: text), let box = try candidate.boundingBox(for: range) else {
                    try fail("OCR found identifying text but could not locate it. Use an explicit --mask region.")
                }
                let normalized = box.boundingBox
                regions.append(CGRect(x: normalized.minX * CGFloat(image.width),
                                      y: normalized.minY * CGFloat(image.height),
                                      width: normalized.width * CGFloat(image.width),
                                      height: normalized.height * CGFloat(image.height)).insetBy(dx: -5, dy: -3))
            }
        }
    }
    return regions
}

let help = """
Capture real CodeTally screenshots with private repository identifiers masked.

Usage (from the repository root):
  swift scripts/capture-screenshots.swift --output docs/screenshots/dashboard-overview.png
  swift scripts/capture-screenshots.swift --output docs/screenshots/repository-detail.png --mask 100,80,300,40
  swift scripts/capture-screenshots.swift --menu-bar --screen-rect X,Y,WIDTH,HEIGHT --output docs/screenshots/menu-bar.png
  swift scripts/capture-screenshots.swift --input /tmp/menu.png --crop X,Y,WIDTH,HEIGHT --menu-bar --output docs/screenshots/menu-bar.png
  swift scripts/capture-screenshots.swift --list-windows

Options:
  --database PATH             Read-only SQLite database; defaults to CodeTally's application data.
  --input PATH                Sanitize an existing PNG without screen capture.
  --crop X,Y,W,H              Crop input pixels from the top-left (requires --input).
  --window-id ID              Capture a particular window; otherwise locate CodeTally's main window.
  --screen-rect X,Y,W,H        Capture a screen rectangle in macOS screen points.
  --menu-bar                  Allow a names-free menu-bar crop (requires --screen-rect or --input with --crop).
  --mask X,Y,W,H              Additional solid mask in OUTPUT PIXELS, top-left origin; repeatable.
  --extra-sensitive-file PATH Additional identifiers, one per line (keep this file outside the repo).
  --output PATH               Sanitized PNG destination; must be under docs/screenshots.
  --list-windows              Print all CodeTally window IDs, layers, and bounds, never titles.
  --audit-input PATH          Audit an existing PNG; report known-identifier match counts only.
  --delay SECONDS             Wait before capture so you can open a native menu (0–60).

Arrange the actual app before each capture. Close unrelated overlays and notifications.
For the menu image, tightly crop only CodeTally's native metrics using --screen-rect.
macOS may require Screen Recording permission for your terminal/host application.
Only private repository names and full owner/name references are masked; public names, standalone
owners, visibility, and metrics remain. Identical public/private repository names cannot be
distinguished by OCR, so a public name identical to a private name is also masked.
OCR can miss truncated or unusual names. Inspect every output at full size, adding --mask regions
for anything missed. Activity titles, usernames, and paths are otherwise unchanged: use additional
masks/identifiers for identifying content before publishing. Raw images are temporary and deleted.
"""

do {
    var args = Array(CommandLine.arguments.dropFirst())
    if args.isEmpty || args.contains("--help") { print(help); exit(0) }
    var values: [String: String] = [:]
    var masks: [CGRect] = []
    var menuBar = false
    var listWindows = false
    while !args.isEmpty {
        let flag = args.removeFirst()
        if flag == "--menu-bar" { menuBar = true; continue }
        if flag == "--list-windows" { listWindows = true; continue }
        guard ["--database", "--input", "--crop", "--window-id", "--screen-rect", "--mask", "--extra-sensitive-file", "--output", "--audit-input", "--delay"].contains(flag), !args.isEmpty else {
            try fail("Unknown option or missing value: \(flag). Use --help.")
        }
        let value = args.removeFirst()
        if flag == "--mask" { masks.append(try rectangle(value)) } else { values[flag] = value }
    }
    if values["--input"] != nil {
        if values["--screen-rect"] != nil { try fail("--input cannot be combined with --screen-rect.") }
        if values["--window-id"] != nil { try fail("--input cannot be combined with --window-id.") }
        if values["--delay"] != nil { try fail("--input cannot be combined with --delay.") }
    }
    if values["--crop"] != nil && values["--input"] == nil {
        try fail("--crop requires --input.")
    }
    if menuBar && values["--screen-rect"] == nil && (values["--input"] == nil || values["--crop"] == nil) {
        try fail("--menu-bar requires --screen-rect or --input with --crop.")
    }
    let windows = (CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []).filter {
        ($0[kCGWindowOwnerName as String] as? String ?? "").lowercased() == "codetally"
    }
    if listWindows {
        for window in windows {
            print("Window \(window[kCGWindowNumber as String] ?? "?") layer \(window[kCGWindowLayer as String] ?? "?"): \(window[kCGWindowBounds as String] ?? "?")")
        }
        if windows.isEmpty { print("No visible CodeTally windows found.") }
        exit(0)
    }
    let database = values["--database"] ?? FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Library/Application Support/com.samfaid.codetally/codetally.sqlite3").path
    guard FileManager.default.fileExists(atPath: database) else { try fail("Database not found. Supply --database using Settings → Data storage.") }
    let temporary = FileManager.default.temporaryDirectory.appendingPathComponent("codetally-screenshots-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: temporary, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
    defer { try? FileManager.default.removeItem(at: temporary) }
    // SQLite may need writable shared-memory files even for a read-only WAL connection.
    // Copy the database and WAL into private storage; never create files beside the source.
    let snapshot = temporary.appendingPathComponent("database.sqlite3")
    let sourcePaths = [database, database + "-wal"]
    func signatures() throws -> [String] {
        try sourcePaths.map { path in
            guard FileManager.default.fileExists(atPath: path) else { return "absent" }
            let attributes = try FileManager.default.attributesOfItem(atPath: path)
            let modified = (attributes[.modificationDate] as? Date)?.timeIntervalSinceReferenceDate ?? -1
            return "\(attributes[.size] ?? "?"):\(modified):\(attributes[.systemFileNumber] ?? "?")"
        }
    }
    let before = try signatures()
    try FileManager.default.copyItem(atPath: database, toPath: snapshot.path)
    if FileManager.default.fileExists(atPath: database + "-wal") {
        try FileManager.default.copyItem(atPath: database + "-wal", toPath: snapshot.path + "-wal")
    }
    guard before == (try signatures()) else {
        try fail("Database changed while creating the private snapshot. Wait for synchronization to finish and retry.")
    }
    let integrity = try run("/usr/bin/sqlite3", [snapshot.path, "PRAGMA integrity_check;"])
    guard String(data: integrity, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines) == "ok" else {
        try fail("Private database snapshot failed its integrity check. Retry after synchronization finishes.")
    }
    let data = try run("/usr/bin/sqlite3", ["-json", snapshot.path,
        "SELECT name AS identifier FROM repositories WHERE is_private = 1 UNION SELECT name_with_owner FROM repositories WHERE is_private = 1;"])
    let rows = data.isEmpty ? [] : try JSONSerialization.jsonObject(with: data) as? [[String: String]] ?? []
    var identifiers = rows.compactMap { $0["identifier"] }.filter { !$0.isEmpty }
    if let extra = values["--extra-sensitive-file"] {
        identifiers += try String(contentsOfFile: extra, encoding: .utf8).components(separatedBy: .newlines).filter { !$0.isEmpty }
    }
    let expressions = try patterns(for: identifiers)
    if let auditInput = values["--audit-input"] {
        let input = URL(fileURLWithPath: auditInput)
        guard let source = CGImageSourceCreateWithURL(input as CFURL, nil), let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
            try fail("Could not decode audit image.")
        }
        let matches = try sensitiveRegions(image, expressions).count
        print("Audit: \(matches) known-identifier OCR matches in \(image.width)×\(image.height) image.")
        print("OCR can miss truncated or unusual names. Visual inspection is still required.")
        try FileManager.default.removeItem(at: temporary)
        exit(matches == 0 ? 0 : 1)
    }
    guard let destination = values["--output"] else { try fail("Specify --output. Use --help.") }
    let output = URL(fileURLWithPath: destination).standardizedFileURL
    let allowed = URL(fileURLWithPath: FileManager.default.currentDirectoryPath).appendingPathComponent("docs/screenshots").standardizedFileURL
    guard output.path.hasPrefix(allowed.path + "/"), output.pathExtension.lowercased() == "png" else {
        try fail("Output must be a PNG under this repository's docs/screenshots directory.")
    }
    let raw = temporary.appendingPathComponent("capture.png")
    let captured: CGImage
    if let input = values["--input"] {
        let inputURL = URL(fileURLWithPath: input)
        guard let source = CGImageSourceCreateWithURL(inputURL as CFURL, nil), let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
            try fail("Could not decode input image.")
        }
        captured = image
    } else {
        var capture = ["-x", "-o"]
        if let screen = values["--screen-rect"] {
            _ = try rectangle(screen)
            capture += ["-R" + screen]
        } else {
            guard !menuBar else { try fail("--menu-bar requires a tightly cropped --screen-rect.") }
            let mainWindow = windows.first { ($0[kCGWindowLayer as String] as? Int) == 0 }
            let id = values["--window-id"] ?? mainWindow?[kCGWindowNumber as String].map { String(describing: $0) }
            guard let id, UInt32(id) != nil else { try fail("No visible CodeTally window found. Open it or specify --window-id.") }
            capture += ["-l", id]
        }
        capture.append(raw.path)
        if let delayText = values["--delay"] {
            guard let delay = Double(delayText), delay.isFinite, delay >= 0, delay <= 60 else {
                try fail("--delay must be a number from 0 to 60 seconds.")
            }
            print("Capturing in \(delay) seconds. Open the desired CodeTally menu now.")
            fflush(stdout)
            Thread.sleep(forTimeInterval: delay)
        }
        _ = try run("/usr/sbin/screencapture", capture)
        guard let source = CGImageSourceCreateWithURL(raw as CFURL, nil), let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
            try fail("Could not decode captured image.")
        }
        captured = image
    }
    var original = captured
    if let cropText = values["--crop"] {
        let crop = try rectangle(cropText)
        let width = CGFloat(captured.width)
        let height = CGFloat(captured.height)
        guard crop.minX >= 0, crop.minY >= 0, crop.maxX <= width, crop.maxY <= height else {
            try fail("The input crop must fall within the input image.")
        }
        guard let cropped = captured.cropping(to: crop) else {
            try fail("Could not crop input image.")
        }
        original = cropped
    }
    let detected = try sensitiveRegions(original, expressions)
    guard identifiers.isEmpty || menuBar || !detected.isEmpty || !masks.isEmpty else {
        try fail("No repository identifiers detected. Refusing output; supply explicit --mask regions after inspecting the live app.")
    }
    guard let context = CGContext(data: nil, width: original.width, height: original.height, bitsPerComponent: 8,
                                  bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
        try fail("Could not create redaction image.")
    }
    let bounds = CGRect(x: 0, y: 0, width: original.width, height: original.height)
    context.draw(original, in: bounds)
    context.setFillColor(CGColor(red: 0.28, green: 0.32, blue: 0.38, alpha: 1))
    for region in detected { context.fill(region.intersection(bounds)) }
    for mask in masks {
        guard bounds.contains(mask) else { try fail("An explicit mask falls outside the output image.") }
        context.fill(CGRect(x: mask.minX, y: CGFloat(original.height) - mask.maxY, width: mask.width, height: mask.height))
    }
    guard let sanitized = context.makeImage() else { try fail("Could not finalize masked image.") }
    guard try sensitiveRegions(sanitized, expressions).isEmpty else {
        try fail("Post-mask OCR still detected identifiers. No output written; add explicit --mask regions.")
    }
    try FileManager.default.createDirectory(at: output.deletingLastPathComponent(), withIntermediateDirectories: true)
    guard let encoded = CGImageDestinationCreateWithURL(output as CFURL, "public.png" as CFString, 1, nil) else {
        try fail("Could not create output PNG.")
    }
    CGImageDestinationAddImage(encoded, sanitized, nil)
    guard CGImageDestinationFinalize(encoded) else { try fail("Could not save output PNG.") }
    print("Saved sanitized \(original.width)×\(original.height) PNG: \(output.lastPathComponent)")
    print("Masked \(detected.count) OCR matches and \(masks.count) explicit regions. Post-mask OCR found no known identifiers.")
    print("Required before publishing: visually inspect the PNG for truncated names and other identifying content.")
} catch {
    fputs("Screenshot capture stopped: \(error)\n", stderr)
    exit(1)
}
