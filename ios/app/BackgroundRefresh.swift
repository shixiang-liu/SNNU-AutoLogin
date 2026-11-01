import Foundation
import BackgroundTasks
import UserNotifications

/// BGAppRefreshTask 注册与续排。
/// 注意：系统不保证执行时机，earliestBeginDate 只是「不早于」。
enum BackgroundRefresh {

    static let taskIdentifier = "cn.edu.snnu.autologin.refresh"

    /// 必须在 App 启动完成前调用（见 SNNUAutoLoginApp.init）
    static func register() {
        _ = BGTaskScheduler.shared.register(
            forTaskWithIdentifier: taskIdentifier,
            using: nil
        ) { task in
            guard let refreshTask = task as? BGAppRefreshTask else {
                task.setTaskCompleted(success: false)
                return
            }
            handle(refreshTask)
        }
    }

    /// 安排下一次刷新。earliestBeginDate 用 15 分钟，实际间隔由系统决定。
    static func scheduleNext() {
        let enabled = UserDefaults.standard.bool(forKey: "autoLoginEnabled")
        guard enabled else {
            BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: taskIdentifier)
            return
        }
        // 先取消同标识符的旧请求，避免重复排队
        BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: taskIdentifier)
        let request = BGAppRefreshTaskRequest(identifier: taskIdentifier)
        request.earliestBeginDate = Date(timeIntervalSinceNow: 15 * 60)
        do {
            try BGTaskScheduler.shared.submit(request)
        } catch {
            // 模拟器上 submit 常失败，真机才有效
            print("BGTaskScheduler submit failed: \(error)")
        }
    }

    private static func handle(_ task: BGAppRefreshTask) {
        // 先续排下一次，避免这次失败后链断掉
        scheduleNext()

        let work = Task {
            defer { task.setTaskCompleted(success: !Task.isCancelled) }
            guard UserDefaults.standard.bool(forKey: "autoLoginEnabled") else { return }
            guard let account = KeychainStore.read("account"),
                  let password = KeychainStore.read("password"),
                  !account.isEmpty else { return }

            let client = PortalClient()
            do {
                let status = try await client.status()
                if !status.isOnline {
                    _ = try await client.login(account: account, password: password, operatorCode: "mobile")
                    await notify(title: "校园网", body: "后台已尝试重新登录")
                }
            } catch {
                await notify(title: "校园网", body: "后台检查失败：\(error.localizedDescription)")
            }
        }

        // 系统给的后台预算约 30 秒，超时前主动收尾
        task.expirationHandler = {
            work.cancel()
        }
    }

    private static func notify(title: String, body: String) async {
        let center = UNUserNotificationCenter.current()
        let settings = await center.notificationSettings()
        guard settings.authorizationStatus == .authorized else { return }
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        let request = UNNotificationRequest(
            identifier: UUID().uuidString,
            content: content,
            trigger: nil
        )
        try? await center.add(request)
    }
}
