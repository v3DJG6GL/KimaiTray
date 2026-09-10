import type { IdlePeriod } from "../api/idleApi";

export function dailyIdleStats(periods: IdlePeriod[], days: number, now = new Date()) {
  return Array.from({ length: days }, (_, index) => {
    const start = new Date(now);
    start.setHours(0, 0, 0, 0);
    start.setDate(start.getDate() - days + index + 1);
    const end = new Date(start);
    end.setDate(end.getDate() + 1);
    const from = start.getTime() / 1000;
    const to = end.getTime() / 1000;
    return {
      date: start,
      seconds: periods.reduce((total, period) => total + Math.max(0,
        Math.min(period.endedAt, to) - Math.max(period.startedAt, from)), 0),
    };
  });
}
