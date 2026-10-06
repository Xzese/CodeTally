#!/usr/bin/env swift
import ApplicationServices
import AppKit
import CoreGraphics
import Foundation
import ImageIO

struct CaptureError: Error, CustomStringConvertible {
    let description: String
}

func fail(_ message: String) throws -> Never { throw CaptureError(description: message) }

func read(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success else { return nil }
    return value
}

func stringAttribute(_ element: AXUIElement, _ name: String) -> String? {
    guard let value = read(element, name) else { return nil }
    return value as? String
}

func element(_ value: CFTypeRef) -> AXUIElement? {
    guard CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
    return unsafeBitCast(value, to: AXUIElement.self)
}

func elements(_ value: CFTypeRef) -> [AXUIElement] {
    if let one = element(value) { return [one] }
    guard CFGetTypeID(value) == CFArrayGetTypeID() else { return [] }
    let array = unsafeBitCast(value, to: CFArray.self)
    return (0..<CFArrayGetCount(array)).compactMap { index in
        guard let pointer = CFArrayGetValueAtIndex(array, index) else { return nil }
        let value = unsafeBitCast(pointer, to: CFTypeRef.self)
        return element(value)
    }
}

func role(_ element: AXUIElement) -> String { stringAttribute(element, kAXRoleAttribute as String) ?? "" }

func childElements(_ element: AXUIElement) -> [AXUIElement] {
    var result: [AXUIElement] = []
    for name in [kAXChildrenAttribute as String, "AXContents", "AXRows", "AXColumns", "AXMenu", "AXMenuBar", "AXExtrasMenuBar"] {
        guard let value = read(element, name) else { continue }
        result.append(contentsOf: elements(value))
    }
    var seen = Set<CFHashCode>()
    return result.filter { seen.insert(CFHash($0)).inserted }
}

func tree(_ roots: [AXUIElement], limit: Int = 12000) -> [AXUIElement] {
    var result: [AXUIElement] = []
    var queue = roots
    var seen = Set<CFHashCode>()
    while !queue.isEmpty && result.count < limit {
        let item = queue.removeFirst()
        guard seen.insert(CFHash(item)).inserted else { continue }
        result.append(item)
        queue.append(contentsOf: childElements(item))
    }
    return result
}

func labels(_ element: AXUIElement) -> [String] {
    [kAXTitleAttribute as String, kAXDescriptionAttribute as String, kAXHelpAttribute as String, kAXValueAttribute as String, "AXLabel"]
        .compactMap { stringAttribute(element, $0) }
}

func matching(_ label: String, in elements: [AXUIElement], exact: Bool = true) -> AXUIElement? {
    let wanted = label.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    return elements.first { element in
        labels(element).contains { candidate in
            let normalized = candidate.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
            return exact ? normalized == wanted : normalized.contains(wanted)
        }
    }
}

func press(_ element: AXUIElement, named label: String) throws {
    guard AXUIElementPerformAction(element, kAXPressAction as CFString) == .success else {
        try fail("macOS could not activate `\(label)`. Check Terminal's Accessibility permission.")
    }
    Thread.sleep(forTimeInterval: 0.65)
}

func windowInfo(_ pid: pid_t) -> (id: UInt32, bounds: CGRect)? {
    let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    return windows.compactMap { entry -> (UInt32, CGRect, Int)? in
        guard (entry[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid,
              let id = (entry[kCGWindowNumber as String] as? NSNumber)?.uint32Value,
              let raw = entry[kCGWindowBounds as String] as? NSDictionary,
              let bounds = CGRect(dictionaryRepresentation: raw),
              (entry[kCGWindowLayer as String] as? NSNumber)?.intValue == 0,
              bounds.width > 300, bounds.height > 300 else { return nil }
        return (id, bounds, Int(bounds.width * bounds.height))
    }.max(by: { $0.2 < $1.2 }).map { (id: $0.0, bounds: $0.1) }
}

func waitForWindow(_ pid: pid_t, timeout: TimeInterval = 20) throws -> (id: UInt32, bounds: CGRect) {
    if let app = NSRunningApplication(processIdentifier: pid) {
        _ = app.activate(options: [.activateAllWindows, .activateIgnoringOtherApps])
    }
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
        if let window = windowInfo(pid) { return window }
        Thread.sleep(forTimeInterval: 0.25)
    }
    try fail("No visible fixture app window found. Check Screen Recording and Accessibility permissions.")
}

func captureWindow(_ pid: pid_t, to output: String) throws {
    let window = try waitForWindow(pid)
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    process.arguments = ["-x", "-l", String(window.id), output]
    try process.run()
    process.waitUntilExit()
    guard process.terminationStatus == 0, (try? Data(contentsOf: URL(fileURLWithPath: output)))?.count ?? 0 > 0 else {
        try fail("Could not capture the fixture app window. Grant Screen Recording access to Terminal and retry.")
    }
}

func frame(_ element: AXUIElement) -> CGRect? {
    func value(_ name: String, type: AXValueType) -> AXValue? {
        guard let raw = read(element, name), CFGetTypeID(raw) == AXValueGetTypeID() else { return nil }
        let value = unsafeBitCast(raw, to: AXValue.self)
        guard AXValueGetType(value) == type else { return nil }
        return value
    }
    if let raw = value("AXFrame", type: .cgRect) {
        var rect = CGRect.zero
        if AXValueGetValue(raw, .cgRect, &rect) { return rect }
    }
    guard let position = value("AXPosition", type: .cgPoint), let size = value("AXSize", type: .cgSize) else { return nil }
    var origin = CGPoint.zero
    var dimensions = CGSize.zero
    guard AXValueGetValue(position, .cgPoint, &origin), AXValueGetValue(size, .cgSize, &dimensions) else { return nil }
    return CGRect(origin: origin, size: dimensions)
}

func nativeRoots(_ app: AXUIElement) -> [AXUIElement] {
    var roots = [app]
    for name in ["AXExtrasMenuBar", "AXMenuBar"] {
        if let value = read(app, name) { roots.append(contentsOf: elements(value)) }
    }
    return roots
}

func statusItem(_ app: AXUIElement, match: String, combined: Bool) throws -> AXUIElement {
    let elements = tree(nativeRoots(app))
    let candidates = elements.filter { ["AXMenuBarItem", "AXButton", "AXMenuExtra"] .contains(role($0)) }
    if let found = matching(match, in: candidates, exact: false) { return found }
    if combined, let onlyStatusItem = candidates.first(where: { labels($0).contains(where: { $0.contains("CodeTally") }) }) { return onlyStatusItem }
    try fail("Could not locate the fixture menu-bar item `\(match)` through Accessibility. Grant Terminal Accessibility access and retry.")
}

func menuFrame(under root: AXUIElement) -> CGRect? {
    tree([root]).compactMap { element -> CGRect? in
        guard role(element) == (kAXMenuRole as String), let rect = frame(element), rect.width > 80, rect.height > 60 else { return nil }
        return rect
    }.max(by: { $0.width * $0.height < $1.width * $1.height })
}

func captureMenu(_ item: AXUIElement, pid: pid_t, output: String, label: String) throws {
    let itemFrame = frame(item)
    try press(item, named: label)
    Thread.sleep(forTimeInterval: 0.4)
    guard let dropdown = menuFrame(under: item), let itemFrame else {
        try fail("The native `\(label)` menu opened, but macOS did not expose its bounds. Enable Accessibility for Terminal and retry.")
    }
    var rect = dropdown.union(itemFrame).insetBy(dx: -8, dy: -8)
    rect.origin.x = max(0, rect.origin.x)
    rect.origin.y = max(0, rect.origin.y)
    let args = [
        "-x", "-R\(Int(rect.origin.x.rounded(.down))),\(Int(rect.origin.y.rounded(.down))),\(Int(rect.width.rounded(.up))),\(Int(rect.height.rounded(.up)))", output,
    ]
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    process.arguments = args
    try process.run()
    process.waitUntilExit()
    guard process.terminationStatus == 0 else { try fail("Could not capture the native menu. Grant Screen Recording access to Terminal and retry.") }
    if let source = CGEventSource(stateID: .hidSystemState),
       let keyDown = CGEvent(keyboardEventSource: source, virtualKey: 53, keyDown: true),
       let keyUp = CGEvent(keyboardEventSource: source, virtualKey: 53, keyDown: false) {
        keyDown.post(tap: .cghidEventTap)
        keyUp.post(tap: .cghidEventTap)
    }
    _ = pid
}

func clickRoute(_ label: String, app: AXUIElement) throws {
    let elements = tree([app])
    let candidates = elements.filter { ["AXButton", "AXRadioButton", "AXMenuItem", "AXTab"].contains(role($0)) }
    guard let control = matching(label, in: candidates) ?? matching(label, in: candidates, exact: false) else {
        try fail("Could not find the accessible `\(label)` control in the fixture app. Verify the app labels or grant Terminal Accessibility access.")
    }
    try press(control, named: label)
}

func clickActivityTab(_ label: String, app: AXUIElement) throws {
    guard let tabs = tree([app]).first(where: { candidate in
        guard role(candidate) == "AXTabGroup" else { return false }
        let text = tree([candidate]).flatMap(labels).joined(separator: " ").lowercased()
        return text.contains("pull requests") && text.contains("issues")
    }) else { try fail("Could not find the activity tabs in the fixture app.") }
    try clickRoute(label, app: tabs)
}

func openRepositoryDetails(_ app: AXUIElement, repository: String) throws {
    let all = tree([app])
    func matchesRepository(_ candidate: AXUIElement) -> Bool {
        tree([candidate]).flatMap(labels).contains { $0.localizedCaseInsensitiveContains(repository) }
    }
    func hasPressAction(_ candidate: AXUIElement) -> Bool {
        var actions: CFArray?
        guard AXUIElementCopyActionNames(candidate, &actions) == .success,
              let actions else { return false }
        return (actions as? [String])?.contains(kAXPressAction as String) == true
    }
    func isFocusable(_ candidate: AXUIElement) -> Bool {
        var settable = DarwinBoolean(false)
        return AXUIElementIsAttributeSettable(candidate, kAXFocusedAttribute as CFString, &settable) == .success && settable.boolValue
    }
    let row = all.first { role($0) == (kAXRowRole as String) && matchesRepository($0) }
    let namedButton = all.first { role($0) == (kAXButtonRole as String) && matchesRepository($0) }
    let actionableGroups = all.filter {
        role($0) == (kAXGroupRole as String) && matchesRepository($0) && (hasPressAction($0) || isFocusable($0))
    }.sorted {
        (tree([$0]).count, frame($0).map { $0.width * $0.height } ?? .greatestFiniteMagnitude)
        < (tree([$1]).count, frame($1).map { $0.width * $0.height } ?? .greatestFiniteMagnitude)
    }
    guard let target = row ?? namedButton ?? actionableGroups.first else {
        try fail("Could not find an actionable fixture repository row `\(repository)` through Accessibility.")
    }
    if AXUIElementPerformAction(target, kAXPressAction as CFString) != .success {
        let focus = AXUIElementSetAttributeValue(target, kAXFocusedAttribute as CFString, kCFBooleanTrue)
        guard focus == .success, let source = CGEventSource(stateID: .hidSystemState),
              let down = CGEvent(keyboardEventSource: source, virtualKey: 36, keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: 36, keyDown: false) else {
            try fail("Could not activate the fixture repository row. Grant Terminal Accessibility access and retry.")
        }
        down.post(tap: .cghidEventTap)
        up.post(tap: .cghidEventTap)
    }
    Thread.sleep(forTimeInterval: 0.8)
}

func scrollToVisible(_ label: String, app: AXUIElement) throws {
    guard let target = matching(label, in: tree([app]), exact: false) else {
        try fail("Could not locate the settings section `\(label)` through Accessibility.")
    }
    _ = AXUIElementPerformAction(target, "AXScrollToVisible" as CFString)
    Thread.sleep(forTimeInterval: 0.4)
}

func launchFixtureApp(bundlePath: String, databasePath: String) throws {
    let bundle = URL(fileURLWithPath: bundlePath, isDirectory: true)
    guard FileManager.default.fileExists(atPath: bundle.path) else { try fail("Fixture app bundle does not exist.") }
    let configuration = NSWorkspace.OpenConfiguration()
    configuration.activates = true
    configuration.createsNewApplicationInstance = true
    configuration.environment = [
        "CODETALLY_SCREENSHOT_MODE": "1",
        "CODETALLY_SCREENSHOT_DATABASE": databasePath,
        "VITE_CODETALLY_SCREENSHOT_MODE": "1",
    ]
    var finished = false
    var launched: NSRunningApplication?
    var launchError: Error?
    NSWorkspace.shared.openApplication(at: bundle, configuration: configuration) { app, error in
        launched = app
        launchError = error
        finished = true
    }
    let deadline = Date().addingTimeInterval(30)
    while !finished && Date() < deadline {
        _ = RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.1))
    }
    guard finished else { try fail("Timed out launching the isolated fixture app.") }
    if let launchError { try fail("Could not launch the fixture app: \(launchError.localizedDescription)") }
    guard let launched, launched.processIdentifier > 0 else { try fail("macOS did not return a fixture app process.") }
    print(launched.processIdentifier)
}

func saveWindow(_ pid: pid_t, _ outputDir: String, _ label: String) throws {
    try captureWindow(pid, to: URL(fileURLWithPath: outputDir).appendingPathComponent(label + ".png").path)
}

do {
    let args = Array(CommandLine.arguments.dropFirst())
    if args.contains("--help") {
        print("Usage: swift scripts/capture-screenshot-fixtures.swift --pid PID --output-dir PATH --mode combined|separate\n       swift scripts/capture-screenshot-fixtures.swift --check-permissions\nAccessibility and Screen Recording permissions may be required.")
        exit(0)
    }
    if args.contains("--check-permissions") {
        print("accessibility=\(AXIsProcessTrusted() ? "granted" : "missing")")
        print("screen_recording=\(CGPreflightScreenCaptureAccess() ? "granted" : "missing")")
        exit(0)
    }
    if let launchIndex = args.firstIndex(of: "--launch-app") {
        guard launchIndex + 1 < args.count,
              let database = args.firstIndex(of: "--database"), database + 1 < args.count else {
            try fail("Use --launch-app BUNDLE --database PATH.")
        }
        try launchFixtureApp(bundlePath: args[launchIndex + 1], databasePath: args[database + 1])
        exit(0)
    }
    guard args.count >= 2 else { try fail("Use --help for usage.") }
    func option(_ flag: String) -> String? {
        guard let index = args.firstIndex(of: flag), index + 1 < args.count else { return nil }
        return args[index + 1]
    }
    guard let pidText = option("--pid"), let pid = pid_t(pidText),
          let outputDir = option("--output-dir"), let mode = option("--mode") else {
        try fail("Missing --pid, --output-dir, or --mode. Use --help.")
    }
    guard ["combined", "separate"].contains(mode) else { try fail("--mode must be combined or separate.") }
    try FileManager.default.createDirectory(atPath: outputDir, withIntermediateDirectories: true)
    _ = try waitForWindow(pid)
    if mode == "combined" {
        let app = AXUIElementCreateApplication(pid)
        try saveWindow(pid, outputDir, "dashboard-overview")
        try scrollToVisible("Repository table", app: app)
        try saveWindow(pid, outputDir, "repositories")
        try openRepositoryDetails(app, repository: "planner")
        try saveWindow(pid, outputDir, "repository-detail")
        try clickRoute("Overview", app: app)
        try scrollToVisible("Visible line categories", app: app)
        try saveWindow(pid, outputDir, "analytics")
        try clickRoute("Overview", app: app)
        for (route, file) in [
            ("Pull requests", "pull-requests"),
            ("Issues", "issues"),
        ] {
            try clickActivityTab(route, app: app)
            try saveWindow(pid, outputDir, file)
        }
        try clickRoute("Kanban Board", app: app)
        try saveWindow(pid, outputDir, "kanban-board")
        try clickRoute("Settings", app: app)
        try scrollToVisible("Combined menu bar preview", app: app)
        try saveWindow(pid, outputDir, "settings")
        let item = try statusItem(app, match: "CodeTally —", combined: true)
        try captureMenu(item, pid: pid, output: URL(fileURLWithPath: outputDir).appendingPathComponent("menu-bar-combined.png").path, label: "combined menu bar")
    } else {
        let app = AXUIElementCreateApplication(pid)
        for (match, file) in [
            ("54.5K lines", "menu-bar-total-lines"),
            ("43.7K source", "menu-bar-source-lines"),
            ("10.8K tests", "menu-bar-test-lines"),
            ("3 PRs", "menu-bar-open-prs"),
            ("2 issues", "menu-bar-open-issues"),
        ] {
            let item = try statusItem(app, match: match, combined: false)
            try captureMenu(item, pid: pid, output: URL(fileURLWithPath: outputDir).appendingPathComponent(file + ".png").path, label: match)
        }
    }
} catch {
    fputs("Screenshot fixture capture failed: \(error)\n", stderr)
    exit(1)
}
