import Foundation

/// 门户客户端：对齐 src/portal.rs 的 HTTP 行为。
/// 只做三件事——登录（5 字段表单 POST）、查状态、断开。
actor PortalClient {

    // MARK: - 与 portal.rs 保持一致的常量

    /// portal.rs:7  pub const HOST
    static let host = "202.117.144.205"

    /// portal.rs:9  端口顺序，首选 8601
    static let ports: [UInt16] = [8601, 8602, 8603, 8605, 8609, 8604, 8606, 8607, 8608, 8610, 8600]

    /// portal.rs:12  const MARK = "当前登录账号"
    static let mark = "当前登录账号"

    /// 状态页可能是 gb2312，用 GB18030 解码（与 portal.rs:111 decode 同一策略）
    static let gb18030: String.Encoding = String.Encoding(
        rawValue: CFStringConvertEncodingToNSStringEncoding(
            CFStringEncoding(CFStringEncodings.GB_18030_2000.rawValue)))

    private let session: URLSession
    private var preferredPort: UInt16 = PortalClient.ports[0]

    init() {
        let cfg = URLSessionConfiguration.ephemeral
        cfg.timeoutIntervalForRequest = 10
        cfg.timeoutIntervalForResource = 20
        cfg.httpShouldSetCookies = true
        cfg.httpCookieAcceptPolicy = .always
        self.session = URLSession(configuration: cfg)
    }

    // MARK: - 结果类型

    enum PortalError: LocalizedError {
        case noReachablePort
        case badResponse(Int)

        var errorDescription: String? {
            switch self {
            case .noReachablePort: return "所有端口都连不上，请确认已连接 SNNU"
            case .badResponse(let code): return "门户返回异常状态码 \(code)"
            }
        }
    }

    struct OnlineStatus: Equatable {
        var isOnline: Bool
        var account: String
        var ip: String
        var loginTime: String
    }

    // MARK: - 端口 failover

    /// 按 portal.rs 的顺序逐个尝试，成功即记住该端口（对应程序里缓存 port 的行为）。
    private func firstReachablePort() async -> UInt16? {
        let ordered = [preferredPort] + PortalClient.ports.filter { $0 != preferredPort }
        for port in ordered {
            if await probe(port: port) {
                preferredPort = port
                return port
            }
        }
        return nil
    }

    private func probe(port: UInt16) async -> Bool {
        guard let url = URL(string: "http://\(Self.host):\(port)/snnuportal/login.jsp") else { return false }
        var req = URLRequest(url: url)
        req.httpMethod = "GET"
        req.timeoutInterval = 5
        do {
            let (_, resp) = try await session.data(for: req)
            guard let http = resp as? HTTPURLResponse else { return false }
            return (200..<400).contains(http.statusCode)
        } catch {
            return false
        }
    }

    // MARK: - 登录

    /// 对齐 portal.rs login_unlocked：POST /snnuportal/login，5 个表单字段。
    /// - Parameter operatorCode: portal.rs 的 yys —— "mobile" / "unicom" / ""
    func login(account: String, password: String, operatorCode: String) async throws -> String {
        guard let port = await firstReachablePort() else { throw PortalError.noReachablePort }

        let base = "http://\(Self.host):\(port)/snnuportal"
        guard let url = URL(string: "\(base)/login") else { throw PortalError.noReachablePort }

        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
        req.setValue("\(base)/login.jsp", forHTTPHeaderField: "Referer")
        req.setValue("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)", forHTTPHeaderField: "User-Agent")

        // 字段顺序与 portal.rs 一致：sourceurl, account, password, yys, issave
        var comps = URLComponents()
        comps.queryItems = [
            URLQueryItem(name: "sourceurl", value: "null"),
            URLQueryItem(name: "account", value: account),
            URLQueryItem(name: "password", value: password),
            URLQueryItem(name: "yys", value: operatorCode),
            URLQueryItem(name: "issave", value: ""),
        ]
        // percentEncodedQuery 保留空值字段（issave=），避免被优化掉
        req.httpBody = comps.percentEncodedQuery?.data(using: .utf8)

        let (data, resp) = try await session.data(for: req)
        guard let http = resp as? HTTPURLResponse else { throw PortalError.badResponse(-1) }
        guard (200..<400).contains(http.statusCode) else { throw PortalError.badResponse(http.statusCode) }

        return Self.decode(data)
    }

    // MARK: - 状态

    /// GET /snnuportal/userstatus.jsp，解析账号 / IP / 登录时间。
    func status() async throws -> OnlineStatus {
        guard let port = await firstReachablePort() else { throw PortalError.noReachablePort }
        guard let url = URL(string: "http://\(Self.host):\(port)/snnuportal/userstatus.jsp") else {
            throw PortalError.noReachablePort
        }

        var req = URLRequest(url: url)
        req.httpMethod = "GET"
        let (data, _) = try await session.data(for: req)
        let html = Self.decode(data)

        // 优先取隐藏输入框（纯 ASCII，不受 gb2312 乱码影响），与快捷指令方案同一策略
        let account = Self.hiddenValue(in: html, id: "account")
        let ip = Self.hiddenValue(in: html, id: "ipaddr")
        let loginTime = Self.firstMatch(in: html, pattern: #"20\d{2}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}"#)

        return OnlineStatus(
            isOnline: !account.isEmpty,
            account: account,
            ip: ip,
            loginTime: loginTime
        )
    }

    // MARK: - 断开

    /// GET /snnuportal/logoff（portal.rs:908），路径无 .jsp 后缀。
    func logoff() async throws -> String {
        guard let port = await firstReachablePort() else { throw PortalError.noReachablePort }
        guard let url = URL(string: "http://\(Self.host):\(port)/snnuportal/logoff") else {
            throw PortalError.noReachablePort
        }
        var req = URLRequest(url: url)
        req.httpMethod = "GET"
        let (data, resp) = try await session.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<400).contains(http.statusCode) else {
            throw PortalError.badResponse((resp as? HTTPURLResponse)?.statusCode ?? -1)
        }
        return Self.decode(data)
    }

    // MARK: - 解析工具

    /// 先试 UTF-8，失败再按 GB18030（与 portal.rs:111 的 decode 一致）
    static func decode(_ data: Data) -> String {
        if let s = String(data: data, encoding: .utf8), !s.isEmpty { return s }
        if let s = String(data: data, encoding: gb18030) { return s }
        return String(decoding: data, as: UTF8.self)
    }

    /// 取 input type="hidden" id="xxx" value="yyy" 的 value
    static func hiddenValue(in html: String, id: String) -> String {
        let pattern = #"id="\#(id)"\s+value="([^"]*)""#
        return firstMatch(in: html, pattern: pattern)
    }

    static func firstMatch(in text: String, pattern: String) -> String {
        guard let re = try? NSRegularExpression(pattern: pattern, options: []) else { return "" }
        let range = NSRange(text.startIndex..<text.endIndex, in: text)
        guard let m = re.firstMatch(in: text, options: [], range: range) else { return "" }
        // 有捕获组取第 1 组，否则取整体
        let idx = m.numberOfRanges > 1 ? 1 : 0
        guard let r = Range(m.range(at: idx), in: text) else { return "" }
        return String(text[r])
    }
}
