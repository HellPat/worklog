import AppKit
import ApplicationServices
import Foundation
func fail(_ message: String) -> Never { fputs(message + "\n", stderr); exit(1) }
let args = Array(CommandLine.arguments.dropFirst())
guard let command = args.first else { fail("Missing command") }
if command == "windows" {
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly,.excludeDesktopElements],kCGNullWindowID) as? [[String:Any]] ?? []
    for w in list where (w[kCGWindowOwnerName as String] as? String ?? "").contains("team-up-gpui") { print(w) }
    exit(0)
}
if command == "check" { print("Accessibility=\(AXIsProcessTrusted()) ScreenCapture=\(CGPreflightScreenCaptureAccess())"); exit(AXIsProcessTrusted() ? 0 : 1) }
if command == "text", args.count == 2, let pid = Int32(args[1]) {
    func walk(_ e: AXUIElement, _ depth: Int) {
        if depth > 20 { return }
        for attr in [kAXTitleAttribute,kAXValueAttribute,kAXDescriptionAttribute] {
            var value:CFTypeRef?
            if AXUIElementCopyAttributeValue(e,attr as CFString,&value) == .success, let v=value { print(v) }
        }
        var children:CFTypeRef?
        if AXUIElementCopyAttributeValue(e,kAXChildrenAttribute as CFString,&children) == .success, let list=children as? [AXUIElement] { for c in list {walk(c,depth+1)} }
    }
    walk(AXUIElementCreateApplication(pid),0);exit(0)
}
guard AXIsProcessTrusted() else { fail("Accessibility permission missing for UI driver") }
if let target = NSWorkspace.shared.runningApplications.first(where: { $0.localizedName == "team-up-gpui-spike" }) {
    target.activate(options:[.activateAllWindows,.activateIgnoringOtherApps]); usleep(200000)
}
func mouse(_ type: CGEventType, _ x: Double, _ y: Double) {
    guard let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: CGPoint(x: x, y: y), mouseButton: .left) else { fail("Cannot create mouse event") }
    event.post(tap: .cghidEventTap)
}
if command == "click", args.count == 3, let x = Double(args[1]), let y = Double(args[2]) {
    mouse(.leftMouseDown, x, y); usleep(60000); mouse(.leftMouseUp, x, y)
} else if command == "drag", args.count == 5, let x = Double(args[1]), let y = Double(args[2]), let endX = Double(args[3]), let endY = Double(args[4]) {
    mouse(.leftMouseDown,x,y)
    for step in 1...20 { usleep(20000); let f = Double(step)/20; mouse(.leftMouseDragged,x+(endX-x)*f,y+(endY-y)*f) }
    usleep(150000); mouse(.leftMouseUp,endX,endY); usleep(150000)
} else if command == "type", args.count == 2 {
    let units = Array(args[1].utf16)
    guard let down = CGEvent(keyboardEventSource:nil,virtualKey:0,keyDown:true), let up = CGEvent(keyboardEventSource:nil,virtualKey:0,keyDown:false) else { fail("Cannot create key event") }
    units.withUnsafeBufferPointer { b in if let p = b.baseAddress { down.keyboardSetUnicodeString(stringLength:b.count,unicodeString:p); up.keyboardSetUnicodeString(stringLength:b.count,unicodeString:p) } }
    down.post(tap:.cghidEventTap); up.post(tap:.cghidEventTap)
} else { fail("Unknown command") }

usleep(200000)
