import AppKit
import ApplicationServices
import Foundation

// Background helper: no windows or activation. Retain OS-returned accessibility values.
let app = NSApplication.shared
app.setActivationPolicy(.prohibited)

func emit(_ value: [String: Any]) {
    let data: Data
    do { data = try JSONSerialization.data(withJSONObject: value) }
    catch {
        let fallback: [String: Any] = ["type": "activity", "event_id": UUID().uuidString,
            "time": Int64(Date().timeIntervalSince1970 * 1000), "application": "macOS", "action": "serialization_fallback",
            "metadata": ["serialization_error": String(describing: error), "native_payload": String(describing: value)]]
        guard let encoded = try? JSONSerialization.data(withJSONObject: fallback) else { return }
        data = encoded
    }
    guard let line = String(data: data, encoding: .utf8) else { return }
    print(line)
    fflush(stdout)
}

if CommandLine.arguments.contains("--request") || CommandLine.arguments.contains("--watch-permission") {
    let request = CommandLine.arguments.contains("--request")
    if request {
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
        _ = AXIsProcessTrustedWithOptions(options)
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") {
            NSWorkspace.shared.open(url)
        }
    }
    var previous: Bool?
    func reportPermission() {
        let authorized = AXIsProcessTrusted()
        if previous != authorized { emit(["type": "status", "authorized": authorized]); previous = authorized }
        if authorized { exit(0) }
    }
    reportPermission()
    let permissionTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { _ in reportPermission() }
    let deadlineTimer = Timer.scheduledTimer(withTimeInterval: 120, repeats: false) { _ in exit(0) }
    app.run()
    _ = (permissionTimer, deadlineTimer)
    exit(0)
}

func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var result: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, name as CFString, &result) == .success else { return nil }
    return result
}
func text(_ element: AXUIElement?, _ name: String) -> String {
    guard let element = element else { return "" }
    if let value = attribute(element, name) as? String { return value }
    if let value = attribute(element, name) as? URL { return value.absoluteString }
    return ""
}
func element(_ parent: AXUIElement, _ name: String) -> AXUIElement? {
    guard let value = attribute(parent, name), CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
    return (value as! AXUIElement)
}
func safeURL(_ value: String) -> String? {
    guard let url = URLComponents(string: value),
          let scheme = url.scheme?.lowercased(), ["http", "https"].contains(scheme),
          url.host != nil else { return nil }
    return url.string
}
func rawValue(_ value: Any) -> Any {
    if value is NSNull { return NSNull() }
    if let value = value as? String { return value }
    if let value = value as? NSNumber {
        return value.doubleValue.isFinite ? value : ["type": "non_finite_number", "native_description": value.description] as Any
    }
    if let value = value as? Date { return ISO8601DateFormatter().string(from: value) }
    if let value = value as? URL { return value.absoluteString }
    if let value = value as? Data { return ["type": "data", "base64": value.base64EncodedString()] }
    if let value = value as? [Any] { return value.map { rawValue($0) } }
    if let value = value as? [String: Any] { return value.mapValues { rawValue($0) } }
    let object = value as CFTypeRef
    if CFGetTypeID(object) == AXValueGetTypeID() {
        let ax = object as! AXValue
        switch AXValueGetType(ax) {
        case .cgPoint:
            var point = CGPoint.zero
            if AXValueGetValue(ax, .cgPoint, &point) { return ["type": "point", "x": point.x, "y": point.y] }
        case .cgSize:
            var size = CGSize.zero
            if AXValueGetValue(ax, .cgSize, &size) { return ["type": "size", "width": size.width, "height": size.height] }
        case .cgRect:
            var rect = CGRect.zero
            if AXValueGetValue(ax, .cgRect, &rect) { return ["type": "rect", "x": rect.origin.x, "y": rect.origin.y, "width": rect.width, "height": rect.height] }
        case .cfRange:
            var range = CFRange(location: 0, length: 0)
            if AXValueGetValue(ax, .cfRange, &range) { return ["type": "range", "location": range.location, "length": range.length] }
        default: break
        }
    }
    // AX references are not a serializable object graph. Preserve their native
    // descriptions instead of dropping them or recursively following cycles.
    return ["type_id": CFGetTypeID(object), "native_description": CFCopyDescription(object) as String? ?? String(describing: value)]
}
func snapshot(_ element: AXUIElement?) -> [String: Any] {
    guard let element = element else { return ["unavailable": true] }
    var names: CFArray?
    let status = AXUIElementCopyAttributeNames(element, &names)
    var result: [String: Any] = ["attribute_names_status": status.rawValue]
    for name in names as? [String] ?? [] {
        var value: CFTypeRef?
        let status = AXUIElementCopyAttributeValue(element, name as CFString, &value)
        result[name] = status == .success && value != nil ? rawValue(value!) : ["unavailable": status.rawValue]
    }
    var actions: CFArray?
    let actionStatus = AXUIElementCopyActionNames(element, &actions)
    result["_actions"] = ["status": actionStatus.rawValue, "names": actions as? [String] ?? []]
    var parameters: CFArray?
    let parameterStatus = AXUIElementCopyParameterizedAttributeNames(element, &parameters)
    result["_parameterized_attributes"] = ["status": parameterStatus.rawValue, "names": parameters as? [String] ?? []]
    return result
}
let browsers = Set(["com.apple.Safari", "com.google.Chrome", "org.mozilla.firefox",
                    "com.microsoft.edgemac", "com.brave.Browser", "company.thebrowser.Browser"])
func browserURL(_ window: AXUIElement, budget: inout Int, depth: Int = 0) -> String? {
    guard budget > 0, depth <= 6 else { return nil }
    budget -= 1
    let role = text(window, "AXRole")
    if role == "AXSecureTextField" { return nil }
    for name in ["AXDocument", "AXURL"] {
        if let value = safeURL(text(window, name)) { return value }
    }
    let description = [text(window, "AXDescription"), text(window, "AXIdentifier")].joined(separator: " ").lowercased()
    // Read AXValue only from an explicitly identified browser address bar.
    if role == "AXTextField" &&
       ["address and search", "address_and_search", "adress- und such", "urlbar", "location bar"].contains(where: { description.contains($0) }),
       let value = safeURL(text(window, "AXValue")) { return value }
    if let children = attribute(window, "AXChildren") as? [AXUIElement] {
        for child in children {
            if let value = browserURL(child, budget: &budget, depth: depth + 1) { return value }
            if budget <= 0 { break }
        }
    }
    return nil
}

var lastSignature = ""
var lastApplication = ""
var lastURL = ""
var lastTrust: Bool?
var monitor: Any?
let system = AXUIElementCreateSystemWide()

func capture(_ action: String, target: AXUIElement? = nil, forced: Bool = false, eventMetadata: [String: Any] = [:]) {
    var targetPID: pid_t = 0
    if let target = target { _ = AXUIElementGetPid(target, &targetPID) }
    guard let running = (targetPID > 0 ? NSRunningApplication(processIdentifier: targetPID) : nil) ?? NSWorkspace.shared.frontmostApplication else { return }
    let trusted = AXIsProcessTrusted()
    let application = AXUIElementCreateApplication(running.processIdentifier)
    let window = trusted ? element(application, "AXFocusedWindow") : nil
    let focused = trusted ? (target ?? element(application, "AXFocusedUIElement")) : nil
    let bundle = running.bundleIdentifier ?? ""
    let role = text(focused, "AXRole")
    let secure = role == "AXSecureTextField"
    let title = secure ? "Secure input" : text(focused, "AXTitle")
    let description = secure ? "" : text(focused, "AXDescription")
    let identifier = secure ? "" : text(focused, "AXIdentifier")
    let windowTitle = text(window, "AXTitle")
    var url: String?
    if trusted, browsers.contains(bundle), let window = window {
        var budget = 160
        url = browserURL(window, budget: &budget)
    }
    let signature = [bundle, windowTitle, url ?? "", role, title, identifier].joined(separator: "\u{1f}")
    guard forced || signature != lastSignature else { return }
    let actualAction: String
    if forced { actualAction = action }
    else if bundle != lastApplication { actualAction = "application" }
    else if (url ?? "") != lastURL { actualAction = "navigation" }
    else { actualAction = "focus" }
    lastSignature = signature
    lastApplication = bundle
    lastURL = url ?? ""
    var event: [String: Any] = [
        "type": "activity", "event_id": UUID().uuidString,
        "time": Int64(Date().timeIntervalSince1970 * 1000),
        "application": running.localizedName ?? "Application",
        "bundle_id": bundle, "window": windowTitle,
        "action": actualAction, "role": role, "target": title,
        "description": description, "identifier": identifier
    ]
    if let url = url { event["url"] = url }
    event["metadata"] = [
        "application": snapshot(trusted ? application : nil), "window": snapshot(window), "target": snapshot(focused),
        "event": eventMetadata,
        "capture": ["accessibility_authorized": trusted, "mouse_monitor_available": monitor != nil, "notification_subscriptions": notificationStatuses],
        "process": ["pid": running.processIdentifier, "bundle_id": bundle,
                    "bundle_url": running.bundleURL?.absoluteString ?? "", "executable_url": running.executableURL?.absoluteString ?? "",
                    "launch_date": running.launchDate.map { ISO8601DateFormatter().string(from: $0) } ?? "",
                    "active": running.isActive, "hidden": running.isHidden, "finished_launching": running.isFinishedLaunching]
    ]
    emit(event)
}

var notificationStatuses: [String: Int32] = [:]
var observerAX: AXObserver?
var observerPID: pid_t = 0
var observedElements = Set<CFHashCode>()
let notifications = ["AXFocusedUIElementChanged", "AXFocusedWindowChanged", "AXWindowCreated", "AXWindowMoved", "AXWindowResized", "AXTitleChanged", "AXValueChanged", "AXSelectedTextChanged", "AXUIElementDestroyed", "AXRowCountChanged", "AXSelectedChildrenChanged", "AXMenuOpened", "AXMenuClosed"]
func updateAXObserver() {
    guard AXIsProcessTrusted(), let running = NSWorkspace.shared.frontmostApplication else { return }
    if observerPID != running.processIdentifier {
        if let observer = observerAX { CFRunLoopRemoveSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(observer), .commonModes) }
        observerAX = nil
        observerPID = running.processIdentifier
        observedElements.removeAll()
        notificationStatuses.removeAll()
        var created: AXObserver?
        let status = AXObserverCreate(observerPID, { _, target, notification, _ in
            capture("accessibility", target: target, forced: true, eventMetadata: ["notification": notification as String])
        }, &created)
        if status == .success, let created = created {
            observerAX = created
            CFRunLoopAddSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(created), .commonModes)
        }
    }
    guard let observer = observerAX else { return }
    let application = AXUIElementCreateApplication(running.processIdentifier)
    let elements = [application, element(application, "AXFocusedWindow"), element(application, "AXFocusedUIElement")].compactMap { $0 }
    for target in elements {
        guard observedElements.insert(CFHash(target)).inserted else { continue }
        for name in notifications {
            notificationStatuses["\(observerPID):\(CFHash(target)):\(name)"] = AXObserverAddNotification(observer, target, name as CFString, nil).rawValue
        }
    }
}
func updatePermission() {
    let trusted = AXIsProcessTrusted()
    let changed = lastTrust != trusted
    if changed {
        lastTrust = trusted
        emit(["type": "status", "authorized": trusted])
    }
    if trusted { updateAXObserver() }
    if changed { capture("permission_changed", forced: true, eventMetadata: ["authorized": trusted]) }
    if monitor == nil {
        monitor = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseUp, .rightMouseUp]) { event in
            guard let point = event.cgEvent?.location else { return }
            var target: AXUIElement?
            let status = AXUIElementCopyElementAtPosition(system, Float(point.x), Float(point.y), &target)
            capture(event.type == .rightMouseUp ? "right_click" : "click", target: target, forced: true,
                        eventMetadata: ["type": event.type.rawValue, "timestamp": event.timestamp,
                                        "modifiers": event.modifierFlags.rawValue, "button": event.buttonNumber,
                                        "click_count": event.clickCount, "pressure": event.pressure,
                                        "x": point.x, "y": point.y, "accessibility_target_status": status.rawValue])
        }
    }
}
updatePermission()
capture("capture_started", forced: true)
let observer = NSWorkspace.shared.notificationCenter.addObserver(
    forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main
) { _ in updatePermission(); capture("application") }
let timer = Timer.scheduledTimer(withTimeInterval: 0.75, repeats: true) { _ in
    updatePermission()
    capture("focus")
}
DispatchQueue.global().async {
    _ = FileHandle.standardInput.readDataToEndOfFile()
    DispatchQueue.main.async {
        capture("capture_stopped", forced: true)
        if let monitor = monitor { NSEvent.removeMonitor(monitor) }
        NSWorkspace.shared.notificationCenter.removeObserver(observer)
        timer.invalidate()
        exit(0)
    }
}
app.run()
