import AppKit

let app = NSApplication.shared
app.setActivationPolicy(.regular)
app.finishLaunching()
let args = CommandLine.arguments
let initial = args.count > 1 ? Double(args[1]) ?? 0 : 0
let title = args.count > 2 ? args[2] : "Time"
let alert = NSAlert()
alert.messageText = title
alert.addButton(withTitle: "Choose")
alert.addButton(withTitle: "Cancel")
let picker = NSDatePicker(frame: NSRect(x: 0, y: 0, width: 250, height: 160))
picker.datePickerStyle = .clockAndCalendar
picker.datePickerElements = [.hourMinute]
picker.timeZone = TimeZone(identifier: "Europe/Berlin")
picker.dateValue = Date(timeIntervalSince1970: initial / 1000)
alert.accessoryView = picker
alert.window.center()
alert.window.makeKeyAndOrderFront(nil)
app.activate(ignoringOtherApps: true)
if alert.runModal() == .alertFirstButtonReturn {
    let calendar = Calendar(identifier: .gregorian)
    let result = calendar.date(bySetting: .second, value: 0, of: picker.dateValue) ?? picker.dateValue
    print(Int64(result.timeIntervalSince1970 / 60) * 60_000)
}
