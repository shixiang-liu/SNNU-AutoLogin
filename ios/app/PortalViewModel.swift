import Foundation
import UserNotifications

/// 界面状态与动作编排。@MainActor 保证所有 @Published 更新都在主线程。
@MainActor
final class PortalViewModel: ObservableObject {

    /// rawValue 直接作为表单 yys 提交：mobile / unicom / 空串（校园网不代拨）
    enum Operator: String, CaseIterable, Identifiable {
        case mobile = "mobile"
        case campus = ""
        case unicom = "unicom"

        var id: String { rawValue }
        var title: String {
            switch self {
            case .mobile: return "移动"
            case .campus: return "校园网"
            case .unicom: return "联通"
            }
        }
    }

    @Published var account: String
    @Published var password: String
    @Published var selectedOperator: Operator = .mobile
    @Published var statusText: String = "未查询"
    @Published var isBusy = false
    @Published var lastError: String?
    @Published var autoLoginEnabled: Bool

    private let client = PortalClient()
    private let defaults = UserDefaults.standard

    init() {
        self.account = KeychainStore.read("account") ?? ""
        self.password = KeychainStore.read("password") ?? ""
        self.autoLoginEnabled = UserDefaults.standard.bool(forKey: "autoLoginEnabled")
    }

    func persistCredentials() {
        KeychainStore.save(account, for: "account")
        KeychainStore.save(password, for: "password")
    }

    func setAutoLogin(_ on: Bool) {
        autoLoginEnabled = on
        defaults.set(on, forKey: "autoLoginEnabled")
        BackgroundRefresh.scheduleNext()
    }

    func login() async {
        isBusy = true
        lastError = nil
        defer { isBusy = false }
        persistCredentials()
        do {
            let body = try await client.login(
                account: account,
                password: password,
                operatorCode: selectedOperator.rawValue
            )
            // 判成功的标志与 portal.rs 一致：返回体包含 MARK
            if body.contains(PortalClient.mark) {
                statusText = "登录成功"
                await refreshStatus()
            } else {
                statusText = "登录失败"
                lastError = PortalClient.firstMatch(in: body, pattern: #"认证失败[^<"]*"#)
            }
        } catch {
            lastError = error.localizedDescription
        }
    }

    func refreshStatus() async {
        isBusy = true
        defer { isBusy = false }
        do {
            let s = try await client.status()
            if s.isOnline {
                statusText = """
                在线
                账号：\(s.account)
                IP：\(s.ip)
                登录时间：\(s.loginTime)
                """
            } else {
                statusText = "当前未登录"
            }
        } catch {
            lastError = error.localizedDescription
        }
    }

    func logoff() async {
        isBusy = true
        defer { isBusy = false }
        do {
            _ = try await client.logoff()
            statusText = "已断开"
        } catch {
            lastError = error.localizedDescription
        }
    }
}
