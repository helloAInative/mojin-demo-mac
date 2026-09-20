import SwiftUI
import AppKit
import UserNotifications

final class NotificationClickHandler: NSObject, UNUserNotificationCenterDelegate {
    var onAction: ((String, String, [String: String]) -> Void)? // code, action, userInfo

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .list, .sound])
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let info = response.notification.request.content.userInfo
        let code = info["code"] as? String ?? ""
        var payload: [String: String] = [:]
        if let m = info as? [String: String] { payload = m }
        let action: String
        switch response.actionIdentifier {
        case "REANALYZE": action = "reanalyze"
        case "DIARY": action = "diary"
        case "STRAT_NOTE": action = "stratNote"
        case "SUGGEST_ORDER": action = "orderTicket"
        case "OPEN_LEVEL": action = "openLevel"
        case "OPEN_SYMBOL":
            action = "open"
        case UNNotificationDefaultActionIdentifier:
            // 默认点击：level 预警通知直接走「跳到价位+草稿委托」，
            // 其它通知仍按原行为（打开标的）。
            action = payload["kind"] == "level" ? "openLevel" : "open"
        default:
            action = payload["kind"] == "level" ? "openLevel" : "open"
        }
        if !code.isEmpty || action != "open" {
            DispatchQueue.main.async { self.onAction?(code, action, payload) }
        }
        completionHandler()
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let store = MarketStore(settings: .shared)
    private var statusItem: NSStatusItem?
    private var popover: NSPopover?
    private var carouselIndex = 0
    private var tick = 0
    private let notifyHandler = NotificationClickHandler()

    func applicationDidFinishLaunching(_ notification: Notification) {
        CrashLog.install()
        AlertService.registerCategories()
        notifyHandler.onAction = { [weak self] code, action, payload in
            Task { @MainActor in
                NotifyGovernor.pendingOpenCode = code
                NotifyGovernor.pendingAction = action
                self?.handleNotify(code: code, action: action, payload: payload)
            }
        }
        UNUserNotificationCenter.current().delegate = notifyHandler
        if store.settings.menuBarOnly {
            NSApp.setActivationPolicy(.accessory)
        } else {
            NSApp.setActivationPolicy(.regular)
        }

        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        if let btn = item.button {
            btn.title = "摸金"
            btn.action = #selector(statusItemClick(_:))
            btn.target = self
            btn.sendAction(on: [.leftMouseUp, .rightMouseUp])
        }
        statusItem = item

        let pop = NSPopover()
        pop.behavior = .transient
        pop.animates = true
        pop.contentSize = NSSize(width: 400, height: 720)
        pop.contentViewController = NSHostingController(
            rootView: ContentView(store: store, settings: store.settings, embeddedInMenu: true)
        )
        popover = pop

        store.start()
        Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.refreshStatusTitle() }
        }

        if !store.settings.menuBarOnly {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { [weak self] in
                self?.showPanel()
            }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        showPanel()
        return true
    }

    func openSymbol(_ code: String) {
        store.switchSymbol(code)
        showPanel()
    }

    private func handleNotify(code: String, action: String, payload: [String: String] = [:]) {
        if !code.isEmpty { store.switchSymbol(code) }
        store.pendingUIAction = action
        if action == "openLevel" {
            store.pendingLevelPayload = payload
        }
        showPanel()
        switch action {
        case "reanalyze":
            store.analyzeWithAI(force: true)
        case "diary":
            let day = MarketStore.todayString()
            store.settings.appendDiaryNote(day: day, note: "通知记复盘 · 价\(String(format: "%.2f", store.quote.price))")
        case "stratNote":
            let day = MarketStore.todayString()
            let why = store.lastStrategyWhy.isEmpty ? "策略触发备注" : store.lastStrategyWhy
            store.settings.appendDiaryNote(day: day, note: "策略备注：\(why.replacingOccurrences(of: "\n", with: " / "))")
        case "orderTicket":
            store.syncTicketFromMarket(forcePrice: store.ticketPrice <= 0)
        case "openLevel":
            // 由 ContentView.onChange(pendingUIAction) 切到 trade tab 时统一消费
            break
        default:
            break
        }
    }

    @objc private func statusItemClick(_ sender: NSStatusBarButton) {
        let event = NSApp.currentEvent
        if event?.type == .rightMouseUp {
            showCarouselMenu()
            return
        }
        // 左键：轮播模式下先切到当前轮播标的，再展开
        if store.settings.menuBarFormat == .carousel {
            let list = store.settings.sortedSymbols
            if !list.isEmpty {
                let sym = list[carouselIndex % list.count]
                if sym.code != store.settings.currentCode {
                    store.switchSymbol(sym.code)
                }
            }
        }
        togglePanel()
    }

    private func showCarouselMenu() {
        let menu = NSMenu()
        for (i, s) in store.settings.sortedSymbols.enumerated() {
            let q = store.watchQuotes[s.code] ?? (s.code == store.settings.currentCode ? store.quote : nil)
            var title = s.name
            if let q, q.price > 0 {
                title = String(format: "%@  %.2f  %@%.1f%%", s.name, q.price, q.pct >= 0 ? "+" : "", q.pct)
            }
            let item = NSMenuItem(title: title, action: #selector(pickCarousel(_:)), keyEquivalent: "")
            item.target = self
            item.tag = i
            if i == carouselIndex { item.state = .on }
            menu.addItem(item)
        }
        statusItem?.menu = menu
        statusItem?.button?.performClick(nil)
        statusItem?.menu = nil
    }

    @objc private func pickCarousel(_ sender: NSMenuItem) {
        let list = store.settings.sortedSymbols
        guard list.indices.contains(sender.tag) else { return }
        carouselIndex = sender.tag
        store.switchSymbol(list[sender.tag].code)
        showPanel()
    }

    @objc private func togglePanel() {
        guard let btn = statusItem?.button, let popover else { return }
        if popover.isShown {
            popover.performClose(nil)
        } else {
            // 复用已有 HostingController，避免每次打开重置 Tab / 草稿 / 滚动位置
            if popover.contentViewController == nil {
                popover.contentViewController = NSHostingController(
                    rootView: ContentView(store: store, settings: store.settings, embeddedInMenu: true)
                )
            }
            popover.show(relativeTo: btn.bounds, of: btn, preferredEdge: .minY)
            NSApp.activate(ignoringOtherApps: true)
        }
    }

    private func showPanel() {
        guard let btn = statusItem?.button, let popover else { return }
        if !popover.isShown {
            if popover.contentViewController == nil {
                popover.contentViewController = NSHostingController(
                    rootView: ContentView(store: store, settings: store.settings, embeddedInMenu: true)
                )
            }
            popover.show(relativeTo: btn.bounds, of: btn, preferredEdge: .minY)
            NSApp.activate(ignoringOtherApps: true)
        }
    }

    @MainActor
    private func refreshStatusTitle() {
        guard let btn = statusItem?.button else { return }
        tick += 1
        let flash = store.menuFlash
        let fmt = store.settings.menuBarFormat
        let title: String

        if fmt == .carousel {
            let list = store.settings.sortedSymbols
            if tick % 6 == 0, !list.isEmpty {
                carouselIndex = (carouselIndex + 1) % list.count
            }
            let sym = list.isEmpty ? store.settings.currentSymbol : list[carouselIndex % max(list.count, 1)]
            let q = store.watchQuotes[sym.code] ?? (sym.code == store.settings.currentCode ? store.quote : nil)
            if let q, q.price > 0 {
                let sign = q.pct >= 0 ? "+" : ""
                title = String(format: "%@ %.2f %@%.1f%%", sym.shortName, q.price, sign, q.pct)
            } else {
                title = sym.shortName
            }
        } else {
            let q = store.quote
            let sym = store.settings.currentSymbol
            if q.price > 0 {
                let sign = q.change >= 0 ? "+" : ""
                switch fmt {
                case .price:
                    title = String(format: "%.2f", q.price)
                case .pricePct:
                    title = String(format: "%.2f %@%.2f%%", q.price, sign, q.pct)
                case .shortName:
                    title = String(format: "%@ %.2f", sym.shortName, q.price)
                case .pctOnly:
                    title = String(format: "%@%.2f%%", sign, q.pct)
                case .carousel:
                    title = "摸金"
                }
            } else {
                title = "摸金"
            }
        }

        let color: NSColor = flash ? .secondaryLabelColor : .labelColor
        btn.attributedTitle = NSAttributedString(string: title, attributes: [
            .font: NSFont.monospacedDigitSystemFont(ofSize: 12, weight: .semibold),
            .foregroundColor: color
        ])
    }
}

@main
struct JJWDWidgetApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

    var body: some Scene {
        Settings {
            SettingsView(store: appDelegate.store, settings: appDelegate.store.settings)
        }
    }
}
