// Native inspection and keyboard events. cliclick owns pointer events;
// CGEvent preserves shortcut modifiers and types Unicode without shortcuts.
// Built into ignored artifacts by the Node runner, using the installed SDK.
import Cocoa
import ApplicationServices

func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var result: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, name as CFString, &result) == .success else { return nil }
    return result
}
func string(_ element: AXUIElement, _ name: String) -> String {
    return attribute(element, name) as? String ?? ""
}
func frame(_ element: AXUIElement) -> [String: Double]? {
    guard let p = attribute(element, kAXPositionAttribute), let s = attribute(element, kAXSizeAttribute),
          CFGetTypeID(p) == AXValueGetTypeID(), CFGetTypeID(s) == AXValueGetTypeID() else { return nil }
    var point = CGPoint.zero
    var size = CGSize.zero
    guard AXValueGetValue(p as! AXValue, .cgPoint, &point), AXValueGetValue(s as! AXValue, .cgSize, &size) else { return nil }
    return ["x": point.x, "y": point.y, "width": size.width, "height": size.height]
}
func displays() -> [[String: Any]] {
    var count: UInt32 = 0
    guard CGGetActiveDisplayList(0, nil, &count) == .success else { return [] }
    var ids = [CGDirectDisplayID](repeating: 0, count: Int(count))
    guard CGGetActiveDisplayList(count, &ids, &count) == .success else { return [] }
    return ids.prefix(Int(count)).enumerated().map { index, id in
        let bounds = CGDisplayBounds(id)
        let mode = CGDisplayCopyDisplayMode(id)
        let screen = NSScreen.screens.first { ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == id }
        let visible = screen?.visibleFrame ?? bounds
        let desktopTop = NSScreen.screens.first?.frame.maxY ?? CGDisplayBounds(CGMainDisplayID()).height
        return ["index": index, "id": id, "x": bounds.minX, "y": bounds.minY,
                "width": bounds.width, "height": bounds.height,
                "workingArea": ["x": visible.minX, "y": desktopTop - visible.maxY, "width": visible.width, "height": visible.height],
                "pixelWidth": mode?.pixelWidth ?? CGDisplayPixelsWide(id),
                "pixelHeight": mode?.pixelHeight ?? CGDisplayPixelsHigh(id)]
    }
}
func setFrame(_ element: AXUIElement, _ rect: [String: Double]) throws {
    var point = CGPoint(x: rect["x"]!, y: rect["y"]!)
    var size = CGSize(width: rect["width"]!, height: rect["height"]!)
    // Move first: macOS can clamp a larger size to the space remaining at
    // the old origin, leaving the recording smaller than a maximized viewer.
    let initialPositionResult = AXUIElementSetAttributeValue(element, kAXPositionAttribute as CFString, AXValueCreate(.cgPoint, &point)!)
    let sizeResult = AXUIElementSetAttributeValue(element, kAXSizeAttribute as CFString, AXValueCreate(.cgSize, &size)!)
    let positionResult = AXUIElementSetAttributeValue(element, kAXPositionAttribute as CFString, AXValueCreate(.cgPoint, &point)!)
    if initialPositionResult != .success || sizeResult != .success || positionResult != .success { throw Failure("Could not place fbro window (AX \(sizeResult.rawValue)/\(positionResult.rawValue))") }
}
struct Failure: Error { let message: String; init(_ message: String) { self.message = message } }
func emit(_ value: Any) throws {
    let data = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    print(String(data: data, encoding: .utf8)!)
}

do {
    let args = Array(CommandLine.arguments.dropFirst())
    let command = args.first ?? "status"
    if command == "status" {
        try emit(["accessibility": AXIsProcessTrusted(), "screenRecording": CGPreflightScreenCaptureAccess(), "displays": displays()])
    } else {
        guard args.count >= 2, let pid = Int32(args[1]), let app = NSRunningApplication(processIdentifier: pid) else { throw Failure("Expected a live application PID") }
        guard AXIsProcessTrusted() else { throw Failure("Accessibility permission is missing for the launching terminal/app") }
        let root = AXUIElementCreateApplication(pid)
        AXUIElementSetMessagingTimeout(root, 3)
        let windows = attribute(root, kAXWindowsAttribute) as? [AXUIElement] ?? []
        if command == "text" {
            guard args.count == 3 else { throw Failure("text requires PID and text") }
            for character in args[2] {
                guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else { throw Failure("Text target lost foreground focus") }
                let units = Array(String(character).utf16)
                for down in [true, false] {
                    guard let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down) else { throw Failure("Could not create text event") }
                    event.flags = []
                    units.withUnsafeBufferPointer { buffer in
                        event.keyboardSetUnicodeString(stringLength: buffer.count, unicodeString: buffer.baseAddress!)
                    }
                    event.post(tap: .cghidEventTap)
                    usleep(20000)
                }
                usleep(40000)
            }
            try emit(["ok": true])
        } else if command == "key" {
            guard args.count >= 3, NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else { throw Failure("Shortcut target is not the foreground fbro process") }
            let codes: [String: CGKeyCode] = ["a":0,"s":1,"d":2,"f":3,"h":4,"g":5,"z":6,"x":7,"c":8,"v":9,"b":11,"q":12,"w":13,"e":14,"r":15,"y":16,"t":17,
                "1":18,"2":19,"3":20,"4":21,"6":22,"5":23,"=":24,"9":25,"7":26,"-":27,"8":28,"0":29,
                "]":30,"o":31,"u":32,"[":33,"i":34,"p":35,"Enter":36,"l":37,"j":38,"'":39,"k":40,";":41,"\\":42,",":43,"/":44,"n":45,"m":46,".":47,"Tab":48," ":49,"`":50,"Backspace":51,"Escape":53,
                "F1":122,"F2":120,"F3":99,"F4":118,"F5":96,"F6":97,"F7":98,"F8":100,"F9":101,"F10":109,"F11":103,"F12":111,
                "Home":115,"End":119,"PageUp":116,"PageDown":121,"Delete":117,"ArrowLeft":123,"ArrowRight":124,"ArrowDown":125,"ArrowUp":126]
            guard let code = codes[args[2]] ?? codes[args[2].lowercased()] else { throw Failure("Unsupported physical shortcut key: \(args[2])") }
            var flags: CGEventFlags = []
            for modifier in args.dropFirst(3) {
                switch modifier {
                case "cmd": flags.insert(.maskCommand)
                case "ctrl": flags.insert(.maskControl)
                case "alt": flags.insert(.maskAlternate)
                case "shift": flags.insert(.maskShift)
                default: throw Failure("Unsupported modifier: \(modifier)")
                }
            }
            for down in [true, false] {
                guard let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down) else { throw Failure("Could not create keyboard event") }
                event.flags = flags
                event.post(tap: .cghidEventTap)
                usleep(20000)
            }
            try emit(["ok": true])
        } else if command == "activate" {
            app.activate(options: [.activateAllWindows])
            try emit(["ok": true])
        } else if command == "place" {
            guard args.count == 6, let x = Double(args[2]), let y = Double(args[3]), let width = Double(args[4]), let height = Double(args[5]), let window = windows.first else { throw Failure("place requires PID x y width height and an existing window") }
            try setFrame(window, ["x": x, "y": y, "width": width, "height": height])
            try emit(frame(window) ?? [:])
        } else if command == "contain" {
            guard args.count == 6, let x = Double(args[2]), let y = Double(args[3]), let width = Double(args[4]), let height = Double(args[5]) else { throw Failure("contain requires PID x y width height") }
            for window in windows {
                guard let rect = frame(window) else { continue }
                if rect["x"]! < x || rect["y"]! < y || rect["x"]! + rect["width"]! > x + width + 1 || rect["y"]! + rect["height"]! > y + height + 1 {
                    let w = min(rect["width"]!, width), h = min(rect["height"]!, height)
                    try setFrame(window, ["x": x + (width - w) / 2, "y": y + (height - h) / 2, "width": w, "height": h])
                }
            }
            try emit(["ok": true])
        } else if command == "inspect" {
            var nodes = [[String: Any]]()
            var visited = Set<CFHashCode>()
            func walk(_ element: AXUIElement, _ depth: Int, _ windowIndex: Int) {
                if depth > 50 || nodes.count >= 12000 { return }
                let hash = CFHash(element)
                if visited.contains(hash) { return }
                visited.insert(hash)
                let role = string(element, kAXRoleAttribute)
                var node: [String: Any] = ["role": role, "title": string(element, kAXTitleAttribute), "description": string(element, kAXDescriptionAttribute),
                    "help": string(element, kAXHelpAttribute), "value": string(element, kAXValueAttribute), "identifier": string(element, kAXIdentifierAttribute),
                    "focused": attribute(element, kAXFocusedAttribute) as? Bool ?? false,
                    "enabled": attribute(element, kAXEnabledAttribute) as? Bool ?? true,
                    "window": windowIndex, "depth": depth]
                if let bounds = frame(element) { node["frame"] = bounds }
                nodes.append(node)
                for child in attribute(element, kAXChildrenAttribute) as? [AXUIElement] ?? [] { walk(child, depth + 1, windowIndex) }
            }
            for (index, window) in windows.enumerated() { walk(window, 0, index) }
            let focusedWindow = attribute(root, kAXFocusedWindowAttribute) as! AXUIElement?
            try emit(["pid": pid, "frontmost": NSWorkspace.shared.frontmostApplication?.processIdentifier == pid,
                      "windows": windows.enumerated().map { index, window in ["index": index, "title": string(window, kAXTitleAttribute), "frame": frame(window) ?? [:]] as [String: Any] },
                      "focusedWindow": focusedWindow.flatMap { target in windows.firstIndex(where: { CFEqual($0, target) }) } ?? 0,
                      "nodes": nodes])
        } else { throw Failure("Unknown native command: \(command)") }
    }
} catch {
    let message = (error as? Failure)?.message ?? String(describing: error)
    FileHandle.standardError.write(Data("\(message)\n".utf8))
    exit(1)
}
