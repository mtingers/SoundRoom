import AppKit
import WebKit
import Foundation
final class Files: NSObject, WKURLSchemeHandler {
    let root: URL
    init(root: String) { self.root=URL(fileURLWithPath:root).standardizedFileURL }
    func webView(_ webView: WKWebView, start urlSchemeTask: WKURLSchemeTask) {
        let request=urlSchemeTask.request.url!,path=request.path=="/" ? "/index.html" : request.path
        let file=root.appendingPathComponent(path).standardizedFileURL
        guard file.path.hasPrefix(root.path+"/"),let data=try? Data(contentsOf:file) else {urlSchemeTask.didReceive(HTTPURLResponse(url:request,statusCode:404,httpVersion:nil,headerFields:[:])!);urlSchemeTask.didFinish();return}
        let mime=["html":"text/html","js":"text/javascript","css":"text/css","json":"application/json"][file.pathExtension] ?? "application/octet-stream"
        urlSchemeTask.didReceive(HTTPURLResponse(url:request,statusCode:200,httpVersion:nil,headerFields:["Content-Type":mime,"Access-Control-Allow-Origin":"*","Cache-Control":"no-store"])!)
        urlSchemeTask.didReceive(data);urlSchemeTask.didFinish()
    }
    func webView(_ webView: WKWebView, stop urlSchemeTask: WKURLSchemeTask) {}
}
final class Runner: NSObject, WKNavigationDelegate, WKScriptMessageHandler, NSWindowDelegate {
    let window: NSWindow
    let browser: WKWebView
    let test: String
    let report: String?
    let keepWindowOpen: Bool
    private var finished = false
    init(base: String, fixture: String, test: String, report: String?, keepWindowOpen: Bool = false) {
        self.report=report
        self.keepWindowOpen=keepWindowOpen
        self.test = test
        let controller = WKUserContentController()
        controller.addUserScript(WKUserScript(source: fixture, injectionTime: .atDocumentStart, forMainFrameOnly: true))
        let configuration = WKWebViewConfiguration(); configuration.websiteDataStore = .nonPersistent(); configuration.userContentController = controller; configuration.setURLSchemeHandler(Files(root: base),forURLScheme:"soundroom-test")
        browser = WKWebView(frame: NSRect(x: 0, y: 0, width: 1360, height: 900), configuration: configuration)
        window = NSWindow(contentRect: NSRect(x: 50, y: 50, width: 1360, height: 900), styleMask: [.titled, .closable], backing: .buffered, defer: false)
        super.init()
        controller.add(self, name: "testResult")
        browser.navigationDelegate = self; window.delegate = self; window.contentView = browser; window.title = "Soundroom test fixture"
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        browser.load(URLRequest(url: URL(string: "soundroom-test://fixture/")!))
        DispatchQueue.main.asyncAfter(deadline: .now() + 30) {
            self.browser.evaluateJavaScript("JSON.stringify({url:location.href,ready:document.readyState,status:document.getElementById('status')?.textContent,errors:window.fixture?.errors})") { state, error in
                self.finish(["ok":false,"failure":"WebKit interaction test timed out",
                             "page":state as? String ?? "unavailable", "inspectionError":error?.localizedDescription ?? ""])
            }
        }
    }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        // The asynchronous script reports through testResult. Returning its
        // Promise is unsupported by evaluateJavaScript's Objective-C bridge.
        webView.evaluateJavaScript("void " + test) { _, error in
            if let error { self.finish(["ok":false,"failure":error.localizedDescription]) }
        }
    }
    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { self.finish(["ok":false,"failure":error.localizedDescription]) }
    func windowWillClose(_ notification: Notification) {
        if !finished { finish(["ok":false,"failure":"Fixture closed before interaction checks completed"]) }
        NSApp.terminate(nil)
    }
    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let result = message.body as? [String: Any] else { print("Invalid test report"); exit(1) }
        finish(result)
    }
    func finish(_ result:[String:Any]) {
        guard !finished else { return }; finished = true
        let data=(try? JSONSerialization.data(withJSONObject:result,options:[.prettyPrinted,.sortedKeys])) ?? Data()
        if let report { try? data.write(to:URL(fileURLWithPath:report),options:.atomic) }
        print(String(decoding:data,as:UTF8.self))
        if keepWindowOpen {
            window.title = (result["ok"] as? Bool)==true ? "Soundroom browser acceptance — Passed" : "Soundroom browser acceptance — Failed"
        } else { exit((result["ok"] as? Bool)==true ? 0 : 1) }
    }
}
let args = CommandLine.arguments
let paths: [String]
if args.count >= 4 {
    paths = Array(args.dropFirst())
} else if let configuration = Bundle.main.object(forInfoDictionaryKey: "SoundroomWebFixture") as? [String: String],
          let base = configuration["assets"], let fixture = configuration["fixture"], let test = configuration["interactions"] {
    paths = [base, fixture, test] + (configuration["report"].map { [$0] } ?? [])
} else {
    fputs("Supply assets, fixture, interactions and optional report paths, or a native fixture bundle configuration.\n", stderr)
    exit(2)
}
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let menu = NSMenu()
let applicationMenu = NSMenu()
let applicationItem = NSMenuItem()
applicationItem.submenu = applicationMenu
menu.addItem(applicationItem)
applicationMenu.addItem(withTitle: "Quit browser fixture", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
app.mainMenu = menu
let runner = Runner(base: paths[0], fixture: try String(contentsOfFile: paths[1], encoding: .utf8), test: try String(contentsOfFile: paths[2], encoding: .utf8), report: paths.count > 3 ? paths[3] : nil,
                    keepWindowOpen: Bundle.main.object(forInfoDictionaryKey: "SoundroomFixtureKeepWindowOpen") as? Bool ?? false)
app.run()
