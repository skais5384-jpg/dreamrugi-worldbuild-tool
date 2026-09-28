/** UTC 저장값은 보존하고, 사용자에게는 현재 OS locale·시간대로 표시한다. */
export function formatLocalDateTime(value: string | null) {
  if (value === null) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "medium",
  }).format(date);
}
