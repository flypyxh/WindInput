//! 统计报表：概览汇总与按日区间。桌面设置页（`wind-webdata` 的 `stats.*`）与移动端
//! （`wind-mobile`）共用这一份算法——两端各算一套，「今日速度」「日均」迟早对不上。
//!
//! 今天的数据一律取采集器内存（始终最新、完整），历史从 store 读：今天的那一行可能还没 flush。

use chrono::Datelike;

use crate::stat_collector::StatCollector;
use crate::stats::{DailyStats, speed_per_minute_ms};
use crate::store::Store;

/// 概览汇总。字段与桌面 `stats.summary` 的 JSON 一一对应。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatsSummary {
    pub today_chars: u32,
    pub today_chinese: u32,
    pub today_english: u32,
    pub total_chars: u64,
    pub active_days: u64,
    pub daily_avg: u64,
    pub streak_current: u32,
    pub streak_max: u32,
    pub week_chars: u64,
    pub month_chars: u64,
    pub max_day_chars: u32,
    pub max_day_date: String,
    pub avg_code_len: f64,
    pub first_select_rate: f64,
    pub today_speed: u32,
    pub overall_speed: u32,
    pub max_speed: u32,
}

/// 今天的日期键（`YYYY-MM-DD`，本地时区，与 `daily_stats` 表的键同形）。
pub fn today_str() -> String {
    chrono::Local::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
}

/// 概览汇总：今日 / 累计 / 本周（周日起）/ 本月，近 90 天的最高日、平均码长、首选率与速度。
pub fn summary(store: &Store, collector: &StatCollector) -> StatsSummary {
    let today_stat = collector.get_today_stat();
    let meta = collector.get_meta();
    let today = today_str();
    let today_total = today_stat.total();

    // 活跃天数（DB 天数；今天有数据但未 flush 时 +1）+ 日均。
    let all = store
        .daily_stats("0000-01-01", "9999-12-31")
        .unwrap_or_default();
    let mut active_days = all.iter().filter(|(_, r)| r.total() > 0).count() as u64;
    let today_in_db = all.iter().any(|(d, _)| d == &today);
    if today_total > 0 && !today_in_db {
        active_days += 1;
    }
    let daily_avg = meta.total_chars.checked_div(active_days).unwrap_or(0);

    // 周（周日起）/ 月统计（YYYY-MM-DD 字典序），今天用内存值。
    let now = chrono::Local::now().date_naive();
    let week_start = now - chrono::Duration::days(now.weekday().num_days_from_sunday() as i64);
    let month_start = now.with_day(1).unwrap_or(now);
    let sum_since = |from: chrono::NaiveDate| -> u64 {
        with_today_mem(
            store,
            &from.format("%Y-%m-%d").to_string(),
            &today,
            &today_stat,
        )
        .iter()
        .map(|(_, r)| r.total() as u64)
        .sum()
    };
    let week_chars = sum_since(week_start);
    let month_chars = sum_since(month_start);

    // 近 90 天：最高日 / 平均码长 / 首选率 / 平均速度。
    let recent_from = (now - chrono::Duration::days(90))
        .format("%Y-%m-%d")
        .to_string();
    let recent = with_today_mem(store, &recent_from, &today, &today_stat);
    let (mut max_day_chars, mut max_day_date) = (0u32, String::new());
    let (mut cl_sum, mut cl_cnt, mut first_sel, mut cand_sel) = (0u64, 0u64, 0u64, 0u64);
    let (mut sp_chars, mut sp_millis) = (0u64, 0u64);
    for (d, r) in &recent {
        let t = r.total();
        if t > max_day_chars {
            max_day_chars = t;
            max_day_date = d.clone();
        }
        cl_sum += r.code_len_sum as u64;
        cl_cnt += r.code_len_count as u64;
        first_sel += r.cand_pos_dist[0] as u64;
        cand_sel += r.cand_pos_dist.iter().map(|&v| v as u64).sum::<u64>();
        // 区间速度：两个分量分别累加后再除一次。逐日算速度再平均是错的——
        // 那会给只打了几十字的日子和整天码字的日子同样的权重。
        let (c, ms) = r.speed_parts();
        sp_chars += c as u64;
        sp_millis += ms;
    }
    let avg_code_len = if cl_cnt > 0 {
        cl_sum as f64 / cl_cnt as f64
    } else {
        0.0
    };
    let first_select_rate = if cand_sel > 0 {
        first_sel as f64 / cand_sel as f64
    } else {
        0.0
    };
    // 系数取自采集器而非配置：`max_speed` 是采集器在 flush 时算好落库的成品值，
    // 两处若各读各的，热改配置后「历史最快」与「今日速度」会差出一个恒定倍数。
    let factor = collector.speed_factor();
    let (today_sp_chars, today_sp_ms) = today_stat.speed_parts();

    StatsSummary {
        today_chars: today_total,
        today_chinese: today_stat.chinese,
        today_english: today_stat.english,
        total_chars: meta.total_chars,
        active_days,
        daily_avg,
        streak_current: meta.streak_current,
        streak_max: meta.streak_max,
        week_chars,
        month_chars,
        max_day_chars,
        max_day_date,
        avg_code_len,
        first_select_rate,
        today_speed: speed_per_minute_ms(today_sp_chars as u64, today_sp_ms, factor),
        overall_speed: speed_per_minute_ms(sp_chars, sp_millis, factor),
        max_speed: meta.max_speed,
    }
}

/// `[from, to]` 每一天的统计（含首尾），缺的日子补零值，今天用采集器内存覆盖。
/// 日期格式 `YYYY-MM-DD`，解析失败返回空表。
pub fn daily_range(
    store: &Store,
    collector: Option<&StatCollector>,
    from: &str,
    to: &str,
) -> anyhow::Result<Vec<(String, DailyStats)>> {
    let mut by_date: std::collections::HashMap<String, DailyStats> =
        store.daily_stats(from, to)?.into_iter().collect();
    let today = today_str();
    if let Some(c) = collector {
        let ts = c.get_today_stat();
        if today.as_str() >= from && today.as_str() <= to && ts.total() > 0 {
            by_date.insert(today, ts);
        }
    }
    let mut out = Vec::new();
    if let (Ok(f), Ok(t)) = (
        chrono::NaiveDate::parse_from_str(from, "%Y-%m-%d"),
        chrono::NaiveDate::parse_from_str(to, "%Y-%m-%d"),
    ) {
        let mut cur = f;
        while cur <= t {
            let key = cur.format("%Y-%m-%d").to_string();
            let rec = by_date.get(&key).cloned().unwrap_or_default();
            out.push((key, rec));
            cur += chrono::Duration::days(1);
        }
    }
    Ok(out)
}

/// 清空全部统计：store 里的历史 + 采集器内存（今日 + 元数据），否则汇总仍读到旧内存值。
pub fn clear(store: Option<&Store>, collector: Option<&StatCollector>) -> anyhow::Result<()> {
    if let Some(store) = store {
        store.clear_stats()?;
    }
    if let Some(c) = collector {
        c.reset();
    }
    Ok(())
}

/// 取 [from, today] 的每日统计，今天用采集器内存值替换/追加（对齐 Go GetSummary 用内存今天）。
fn with_today_mem(
    store: &Store,
    from: &str,
    today: &str,
    today_stat: &DailyStats,
) -> Vec<(String, DailyStats)> {
    let mut days = store.daily_stats(from, today).unwrap_or_default();
    let mut has = false;
    for (d, r) in days.iter_mut() {
        if d == today {
            *r = today_stat.clone();
            has = true;
        }
    }
    if !has && today >= from && today_stat.total() > 0 {
        days.push((today.to_string(), today_stat.clone()));
    }
    days
}
