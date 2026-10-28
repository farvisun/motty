import AppKit
import ScreenCaptureKit
// usage: rectshot <x> <y> <w> <h> <out.png> <window-id>...
// Captures a screen rect (points, top-left origin) at the display's scale,
// composited from the listed windows only, so whatever else is on screen
// (other apps, overlays above the window) never ends up in the image.
let a = CommandLine.arguments
guard a.count >= 7, let x = Double(a[1]), let y = Double(a[2]), let w = Double(a[3]), let h = Double(a[4]) else {
    FileHandle.standardError.write("usage: rectshot x y w h out.png window-id...\n".data(using: .utf8)!)
    exit(2)
}
let out = URL(fileURLWithPath: a[5])
let ids = Set(a[6...].compactMap { CGWindowID($0) })

func fail(_ message: String) -> Never {
    FileHandle.standardError.write("rectshot: \(message)\n".data(using: .utf8)!)
    exit(1)
}

Task {
    do {
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        let windows = content.windows.filter { ids.contains($0.windowID) }
        guard windows.count == ids.count else { fail("windows not on screen") }
        let rect = CGRect(x: x, y: y, width: w, height: h)
        guard let display = content.displays.first(where: { $0.frame.intersects(rect) }) else { fail("no display") }
        let scale = NSScreen.screens.first(where: { $0.frame.width == display.frame.width })?.backingScaleFactor ?? 2
        let filter = SCContentFilter(display: display, including: windows)
        let config = SCStreamConfiguration()
        config.sourceRect = rect.offsetBy(dx: -display.frame.minX, dy: -display.frame.minY)
        config.width = Int(w * scale)
        config.height = Int(h * scale)
        config.showsCursor = false
        config.captureResolution = .best
        let image = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config)
        let rep = NSBitmapImageRep(cgImage: image)
        guard let png = rep.representation(using: .png, properties: [:]) else { fail("png encoding") }
        try png.write(to: out)
        exit(0)
    } catch {
        fail("\(error)")
    }
}
RunLoop.main.run()
