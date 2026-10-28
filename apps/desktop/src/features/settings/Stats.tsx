// Settings → Stats: a year of your email, counted on this Mac (api.mailStats,
// penguin-core store_stats.rs). Headline tiles, a GitHub-style day grid of
// sends (or received mail), when in the week you write, reply speed, and the
// people you trade the most mail with. OWNER: settings agent.
import { useEffect, useMemo, useState, type MouseEvent } from "react";
import { api, asCommandError } from "../../lib/api";
import { meta } from "../../app/store";
import { accountName } from "../../components/Identity";
import { Select } from "../../components/Select";
import { num, shortDate } from "../../lib/format";
import type { MailStats, StatsDay, StatsPerson } from "../../lib/types";
import { Choice, Section } from "./parts";
import "./stats.css";

type Measure = "sent" | "received";
const ALL = "~all";
const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

export function StatsSection() {
  const accounts = meta.use((m) => m.accounts);
  const [account, setAccount] = useState<string>(ALL);
  const [measure, setMeasure] = useState<Measure>("sent");
  const [stats, setStats] = useState<MailStats | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    setError(null);
    api.mailStats(account === ALL ? null : [account]).then(
      (s) => live && setStats(s),
      (e) => live && setError(asCommandError(e).message),
    );
    return () => {
      live = false;
    };
  }, [account]);

  const options = useMemo(
    () => [{ value: ALL, label: "All accounts" }, ...accounts.map((a) => ({ value: a.id, label: accountName(a, accounts) }))],
    [accounts],
  );

  return (
    <Section
      id="stats"
      icon="chart"
      title="Stats"
      badge={accounts.length > 1 ? <Select value={account} options={options} onChange={setAccount} label="Account" className="stats-account" /> : undefined}
    >
      {error ? (
        <p className="st-muted">Couldn't count your mail: {error}</p>
      ) : !stats ? (
        <p className="st-muted">Counting…</p>
      ) : (
        <StatsBody stats={stats} measure={measure} setMeasure={setMeasure} />
      )}
    </Section>
  );
}

function StatsBody({ stats, measure, setMeasure }: { stats: MailStats; measure: Measure; setMeasure: (m: Measure) => void }) {
  const busiest = stats.days.reduce<StatsDay | null>((b, d) => (!b || d.sent > b.sent ? d : b), null);
  const perDay = stats.sent / Math.max(1, stats.days.length);
  return (
    <>
      <div className="stats-tiles">
        <Tile value={num(stats.sent)} label="Sent this year" hint={`${perDay < 10 ? perDay.toFixed(1) : Math.round(perDay)} a day on average`} />
        <Tile value={num(stats.received)} label="Received" hint={stats.received ? `${Math.round((stats.receivedNewsletters / stats.received) * 100)}% newsletters and bulk` : undefined} />
        <Tile value={dayCount(stats.currentStreak)} label="Current streak" hint={`Longest ${dayCount(stats.longestStreak)}`} />
        <Tile
          value={stats.medianReplyMinutes === null ? "–" : duration(stats.medianReplyMinutes)}
          label="Typical reply time"
          hint={stats.replies ? `${Math.round((stats.repliesWithinHour / stats.replies) * 100)}% of ${num(stats.replies)} replies within an hour` : "No replies yet"}
        />
      </div>

      <div className="st-sub-row">
        <h3 className="st-sub">Every day this year</h3>
        <span className="grow" />
        <Choice
          label="Show"
          value={measure}
          onChange={setMeasure}
          options={[
            { value: "sent", label: "Sent" },
            { value: "received", label: "Received" },
          ]}
        />
      </div>
      <DayGrid days={stats.days} measure={measure} />
      <p className="st-muted stats-caption">
        {num(stats.activeDays)} days with something sent
        {busiest && busiest.sent > 0 ? ` · busiest ${longDate(busiest.date)} with ${num(busiest.sent)}` : ""}
      </p>

      <h3 className="st-sub">When you write</h3>
      <HourGrid byHour={stats.sentByHour} />
      <p className="st-muted stats-caption">{peak(stats.sentByHour)}</p>

      <div className="stats-people">
        <People title="You write to" people={stats.topRecipients} empty="Nothing sent this year" />
        <People title="Writes to you" people={stats.topSenders} empty="No mail from people this year" />
      </div>

      <p className="st-muted stats-foot">
        Counted from the mail on this Mac{stats.oldestMessage ? `, which goes back to ${shortDate(stats.oldestMessage)}` : ""}. Nothing here leaves your computer.
      </p>
    </>
  );
}

function Tile({ value, label, hint }: { value: string; label: string; hint?: string }) {
  return (
    <div className="stats-tile">
      <div className="stats-tile-value tnum">{value}</div>
      <div className="stats-tile-label">{label}</div>
      {hint && <div className="st-muted">{hint}</div>}
    </div>
  );
}

/** Hover tooltip shared by both grids: text plus where to put it. */
type Tip = { text: string; x: number; y: number } | null;

function useTip() {
  const [tip, setTip] = useState<Tip>(null);
  const show = (text: string) => (e: MouseEvent<HTMLElement>) => {
    const cell = e.currentTarget.getBoundingClientRect();
    const box = e.currentTarget.closest<HTMLElement>(".stats-plot")!.getBoundingClientRect();
    setTip({ text, x: cell.left - box.left + cell.width / 2, y: cell.top - box.top });
  };
  const el = tip && (
    <div className="stats-tip" role="tooltip" style={{ left: tip.x, top: tip.y }}>
      {tip.text}
    </div>
  );
  return { show, hide: () => setTip(null), el };
}

/** 0–4: empty, then quartiles of the year's non-zero days (GitHub's buckets). */
function levels(values: number[]): (n: number) => number {
  const nz = values.filter((v) => v > 0).sort((a, b) => a - b);
  if (!nz.length) return () => 0;
  const q = (p: number) => nz[Math.min(nz.length - 1, Math.floor(p * nz.length))];
  const [a, b, c] = [q(0.25), q(0.5), q(0.75)];
  return (n) => (n <= 0 ? 0 : n <= a ? 1 : n <= b ? 2 : n <= c ? 3 : 4);
}

function DayGrid({ days, measure }: { days: StatsDay[]; measure: Measure }) {
  const { show, hide, el } = useTip();
  const level = useMemo(() => levels(days.map((d) => d[measure])), [days, measure]);
  // Columns are weeks starting Sunday; pad the first week up to the first day.
  const lead = days.length ? parseDay(days[0].date).getDay() : 0;
  const cells: (StatsDay | null)[] = [...Array<null>(lead).fill(null), ...days];
  const weeks = Math.ceil(cells.length / 7);
  const months: { col: number; label: string }[] = [];
  for (let w = 0; w < weeks; w++) {
    const first = cells.slice(w * 7, w * 7 + 7).find((d): d is StatsDay => d !== null);
    if (!first) continue;
    const m = parseDay(first.date).getMonth();
    if (!months.length || months[months.length - 1].label !== MONTHS[m]) months.push({ col: w, label: MONTHS[m] });
  }
  // No squashed label at either end.
  if (months.length > 1 && months[1].col - months[0].col < 3) months.shift();
  if (months.length && weeks - months[months.length - 1].col < 3) months.pop();
  const verb = measure === "sent" ? "sent" : "received";
  return (
    <div className="stats-plot stats-days" onMouseLeave={hide}>
      <div className="stats-days-months" style={{ gridTemplateColumns: `repeat(${weeks}, minmax(0, 1fr))` }} aria-hidden>
        {months.map((m) => (
          <span key={m.col} style={{ gridColumn: `${m.col + 1} / span 3` }}>
            {m.label}
          </span>
        ))}
      </div>
      <div className="stats-days-body">
        <div className="stats-days-wd" aria-hidden>
          <span />
          <span>Mon</span>
          <span />
          <span>Wed</span>
          <span />
          <span>Fri</span>
          <span />
        </div>
        <div className="stats-days-cells" style={{ gridTemplateColumns: `repeat(${weeks}, minmax(0, 1fr))` }} role="img" aria-label={`Messages ${verb} each day this year`}>
          {cells.map((d, i) =>
            d ? (
              <span
                key={d.date}
                className="stats-cell"
                data-l={level(d[measure])}
                onMouseEnter={show(`${d[measure] ? num(d[measure]) : "Nothing"} ${verb} · ${longDate(d.date)}`)}
              />
            ) : (
              <span key={`pad${i}`} />
            ),
          )}
        </div>
      </div>
      <div className="stats-legend st-muted" aria-hidden>
        Less
        {[0, 1, 2, 3, 4].map((l) => (
          <span key={l} className="stats-cell" data-l={l} />
        ))}
        More
      </div>
      {el}
    </div>
  );
}

function HourGrid({ byHour }: { byHour: number[][] }) {
  const { show, hide, el } = useTip();
  const max = Math.max(1, ...byHour.flat());
  return (
    <div className="stats-plot stats-hours" onMouseLeave={hide} role="img" aria-label="Messages sent by weekday and hour">
      {byHour.map((row, wd) => (
        <div className="stats-hours-row" key={wd}>
          <span className="stats-hours-wd">{WEEKDAYS[wd]}</span>
          {row.map((n, h) => (
            <span key={h} className="stats-dot-hit" onMouseEnter={show(`${n ? num(n) : "Nothing"} sent · ${WEEKDAYS[wd]} ${hourRange(h)}`)}>
              {n > 0 && <span className="stats-dot" style={{ transform: `scale(${0.25 + 0.75 * Math.sqrt(n / max)})` }} />}
            </span>
          ))}
        </div>
      ))}
      <div className="stats-hours-row stats-hours-axis" aria-hidden>
        <span className="stats-hours-wd" />
        {Array.from({ length: 24 }, (_, h) => (
          <span key={h}>{h % 6 === 0 ? hourLabel(h) : ""}</span>
        ))}
      </div>
      {el}
    </div>
  );
}

function People({ title, people, empty }: { title: string; people: StatsPerson[]; empty: string }) {
  const max = Math.max(1, ...people.map((p) => p.count));
  return (
    <div>
      <h3 className="st-sub">{title}</h3>
      {people.length === 0 ? (
        <p className="st-muted">{empty}</p>
      ) : (
        <ol className="stats-people-list">
          {people.map((p) => (
            <li key={p.email} title={p.email}>
              <span className="stats-person">{p.name ?? p.email}</span>
              <span className="stats-bar" style={{ width: `${(p.count / max) * 100}%` }} />
              <span className="stats-count tnum">{num(p.count)}</span>
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}

function parseDay(iso: string): Date {
  const [y, m, d] = iso.split("-").map(Number);
  return new Date(y, m - 1, d);
}

function longDate(iso: string): string {
  return parseDay(iso).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
}

function dayCount(n: number): string {
  return `${num(n)} ${n === 1 ? "day" : "days"}`;
}

function duration(minutes: number): string {
  if (minutes < 60) return `${minutes} min`;
  if (minutes < 48 * 60) return `${Math.round((minutes / 60) * 10) / 10} h`;
  return `${Math.round(minutes / 1440)} days`;
}

function hourLabel(h: number): string {
  return new Date(2000, 0, 1, h).toLocaleTimeString(undefined, { hour: "numeric" });
}

function hourRange(h: number): string {
  return `${hourLabel(h)}–${hourLabel((h + 1) % 24)}`;
}

/** "You write most on Tuesdays, around 10 AM." */
function peak(byHour: number[][]): string {
  const total = byHour.flat().reduce((a, b) => a + b, 0);
  if (!total) return "Nothing sent this year yet.";
  const days = byHour.map((r) => r.reduce((a, b) => a + b, 0));
  const hours = Array.from({ length: 24 }, (_, h) => byHour.reduce((a, r) => a + r[h], 0));
  const wd = days.indexOf(Math.max(...days));
  const h = hours.indexOf(Math.max(...hours));
  const night = hours.slice(22).concat(hours.slice(0, 5)).reduce((a, b) => a + b, 0);
  const weekend = days[5] + days[6];
  const extras = [
    weekend ? `${Math.round((weekend / total) * 100)}% on weekends` : "never on weekends",
    night ? `${Math.round((night / total) * 100)}% late at night (10 PM–5 AM)` : null,
  ].filter(Boolean);
  const dayName = new Date(2001, 0, 1 + wd).toLocaleDateString(undefined, { weekday: "long" });
  return `You write most on ${dayName}s, around ${hourLabel(h)}; ${extras.join(", ")}.`;
}
