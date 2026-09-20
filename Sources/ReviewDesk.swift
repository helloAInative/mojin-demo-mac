import Foundation

/// 复盘工作台：按日拼接信号 + 委托照抄 + 日记
enum TradeStoryDesk {
    static func dayString(_ date: Date = Date()) -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "Asia/Shanghai")
        f.dateFormat = "yyyy-MM-dd"
        return f.string(from: date)
    }

    static func clock(_ date: Date) -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "Asia/Shanghai")
        f.dateFormat = "HH:mm"
        return f.string(from: date)
    }

    static func build(
        day: String,
        code: String,
        name: String,
        signals: [SignalEvent],
        tickets: [SemiOrderTicket],
        diary: String
    ) -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "Asia/Shanghai")
        f.dateFormat = "yyyy-MM-dd"

        let sigs = signals.filter { f.string(from: $0.at) == day && (code.isEmpty || $0.code == code) }
            .sorted { $0.at < $1.at }
        let ords = tickets.filter { f.string(from: $0.at) == day && (code.isEmpty || $0.code == code) }
            .sorted { $0.at < $1.at }

        var lines: [String] = []
        lines.append("## 交易故事 · \(day) · \(name.isEmpty ? code : name) (\(code))")
        lines.append("")

        if !diary.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            lines.append("### 日记")
            lines.append(diary.trimmingCharacters(in: .whitespacesAndNewlines))
            lines.append("")
        }

        if !sigs.isEmpty {
            lines.append("### 信号时间线")
            for s in sigs {
                lines.append("- \(clock(s.at)) [\(s.kind)] \(s.title) — \(s.body)")
                if !s.why.isEmpty {
                    lines.append("  为何：\(s.why)")
                }
            }
            lines.append("")
        }

        if !ords.isEmpty {
            lines.append("### 半自动委托照抄")
            for o in ords {
                let flag = o.filled ? "已勾成交" : (o.copied ? "已复制" : "草稿")
                lines.append("- \(clock(o.at)) \(o.oneLine) · \(flag)")
                if !o.note.isEmpty { lines.append("  注：\(o.note)") }
            }
            lines.append("")
        }

        if diary.isEmpty && sigs.isEmpty && ords.isEmpty {
            lines.append("_当日暂无日记 / 信号 / 委托记录_")
        } else {
            lines.append("---")
            lines.append("共 \(sigs.count) 条信号 · \(ords.count) 条委托 · 日记\(diary.isEmpty ? "无" : "有")")
        }
        return lines.joined(separator: "\n")
    }
}
