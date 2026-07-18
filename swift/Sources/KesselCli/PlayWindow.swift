import AppKit
import Foundation
import AgentBridge

// Phase 2 of the fantasy-console VM: a native, human-playable window. `kessel
// --play <file.ux|file.asm>` loads a ROM into a standalone `VmPlayer` (no LLM
// needed) and renders its 128x128 framebuffer scaled up, ticking at 60 Hz and
// mapping the keyboard to the gamepad. The same ROMs the model authors headless
// are playable here.

/// Gamepad button bits — must match `device.rs`.
private enum Btn {
    static let left: UInt8 = 0x01
    static let right: UInt8 = 0x02
    static let up: UInt8 = 0x04
    static let down: UInt8 = 0x08
    static let a: UInt8 = 0x10
    static let b: UInt8 = 0x20
    static let start: UInt8 = 0x40
    static let select: UInt8 = 0x80
}

/// A view that draws the current RGBA framebuffer with nearest-neighbour scaling
/// and tracks pressed keys as a gamepad bitfield.
final class PixelView: NSView {
    var image: CGImage?
    private(set) var pressed: UInt8 = 0

    // Top-left origin so the framebuffer's first row renders at the top.
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        ctx.setFillColor(NSColor.black.cgColor)
        ctx.fill(bounds)
        if let img = image {
            ctx.interpolationQuality = .none
            ctx.draw(img, in: bounds)
        }
    }

    private func bit(for keyCode: UInt16) -> UInt8? {
        switch keyCode {
        case 123: return Btn.left
        case 124: return Btn.right
        case 126: return Btn.up
        case 125: return Btn.down
        case 6: return Btn.a       // z
        case 7: return Btn.b       // x
        case 36: return Btn.start  // return
        case 49: return Btn.select // space
        default: return nil
        }
    }

    override func keyDown(with event: NSEvent) {
        if event.isARepeat { return }
        if let b = bit(for: event.keyCode) { pressed |= b } else { super.keyDown(with: event) }
    }

    override func keyUp(with event: NSEvent) {
        if let b = bit(for: event.keyCode) { pressed &= ~b } else { super.keyUp(with: event) }
    }

    /// Clear held keys when focus is lost, so nothing "sticks".
    override func resignFirstResponder() -> Bool {
        pressed = 0
        return true
    }
}

/// Drives one `VmPlayer` at 60 Hz into a `PixelView`.
final class PlayController {
    private let player: VmPlayer
    private let view: PixelView
    private let dim: Int
    private var timer: Timer?

    init(player: VmPlayer, view: PixelView) {
        self.player = player
        self.view = view
        self.dim = Int(player.screenDim())
    }

    func start() {
        let t = Timer(timeInterval: 1.0 / 60.0, repeats: true) { [weak self] _ in
            self?.tick()
        }
        RunLoop.main.add(t, forMode: .common) // keep ticking during window resize/menus
        timer = t
    }

    func stop() {
        timer?.invalidate()
        timer = nil
    }

    private func tick() {
        player.tick(buttons: view.pressed)
        if let data = player.framebufferRgba() {
            view.image = Self.makeImage(data: data, dim: dim)
            view.needsDisplay = true
        }
    }

    private static func makeImage(data: Data, dim: Int) -> CGImage? {
        guard data.count == dim * dim * 4, let provider = CGDataProvider(data: data as CFData) else {
            return nil
        }
        return CGImage(
            width: dim,
            height: dim,
            bitsPerComponent: 8,
            bitsPerPixel: 32,
            bytesPerRow: dim * 4,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
            provider: provider,
            decode: nil,
            shouldInterpolate: false,
            intent: .defaultIntent
        )
    }
}

/// Quit the app when the game window closes.
final class PlayAppDelegate: NSObject, NSApplicationDelegate {
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

// Retain the AppKit objects for the process lifetime (main-thread only).
nonisolated(unsafe) private var playObjects: [Any] = []

/// Load `romPath` into a standalone player and run the game window. Blocks in the
/// AppKit run loop and exits the process when the window closes; never returns.
@MainActor
func runPlayMode(romPath: String) -> Never {
    let source: String
    do {
        source = try String(contentsOfFile: romPath, encoding: .utf8)
    } catch {
        FileHandle.standardError.write(Data("Cannot read '\(romPath)': \(error)\n".utf8))
        exit(1)
    }

    let player = VmPlayer()
    let error = player.load(source: source, path: romPath)
    if !error.isEmpty {
        FileHandle.standardError.write(Data("Failed to load '\(romPath)':\n\(error)\n".utf8))
        exit(1)
    }

    let app = NSApplication.shared
    app.setActivationPolicy(.regular)

    let dim = Int(player.screenDim())
    let scale = 5
    let side = CGFloat(dim * scale)
    let frame = NSRect(x: 0, y: 0, width: side, height: side)

    let view = PixelView(frame: frame)
    let window = NSWindow(
        contentRect: frame,
        styleMask: [.titled, .closable, .miniaturizable],
        backing: .buffered,
        defer: false
    )
    window.title = "Kessel — \(URL(fileURLWithPath: romPath).lastPathComponent)"
    window.contentView = view
    window.center()
    window.isReleasedWhenClosed = false
    window.makeKeyAndOrderFront(nil)
    window.makeFirstResponder(view)

    let controller = PlayController(player: player, view: view)
    let delegate = PlayAppDelegate()
    app.delegate = delegate
    playObjects = [controller, delegate, window]

    print("Playing \(romPath) — arrows move, Z/X = A/B, Return/Space = Start/Select. Close the window to quit.")
    controller.start()
    app.activate(ignoringOtherApps: true)
    app.run()
    exit(0)
}
