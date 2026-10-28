import AppKit
// usage: backdrop <x> <y> <w> <h> <rrggbb>
// Shows a borderless window of one solid color over the given screen rect
// (points, top-left origin) until killed. Parity captures put it behind the
// window under test so behind-window materials blur a known color and
// nothing else on screen ends up in the capture. Each CGWindowID read from
// stdin re-orders the backdrop directly below that window (activating an app
// raises all of its windows over the backdrop).
let a = CommandLine.arguments
guard a.count == 6, let x = Double(a[1]), let y = Double(a[2]), let w = Double(a[3]), let h = Double(a[4]),
      let rgb = UInt32(a[5], radix: 16) else {
    FileHandle.standardError.write("usage: backdrop x y w h rrggbb\n".data(using: .utf8)!)
    exit(2)
}
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let screenHeight = NSScreen.screens[0].frame.height
let frame = NSRect(x: x, y: screenHeight - y - h, width: w, height: h)
let window = NSWindow(contentRect: frame, styleMask: .borderless, backing: .buffered, defer: false)
window.backgroundColor = NSColor(srgbRed: CGFloat((rgb >> 16) & 0xff) / 255, green: CGFloat((rgb >> 8) & 0xff) / 255,
                                 blue: CGFloat(rgb & 0xff) / 255, alpha: 1)
window.isOpaque = true
window.hasShadow = false
window.title = "motty-backdrop"
window.orderFrontRegardless()
print("ready")
fflush(stdout)
DispatchQueue.global().async {
    while let line = readLine() {
        guard let id = Int(line.trimmingCharacters(in: .whitespaces)) else { continue }
        DispatchQueue.main.async {
            window.order(.below, relativeTo: id)
            print("ordered")
            fflush(stdout)
        }
    }
}
app.run()
