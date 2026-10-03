import type { RecoveryChange, RecoveryContent } from "../bridge/workspace";
import type { Value } from "../bridge/types";
import { ValueRead } from "./FieldValue";
import { Checkbox } from "../ui/Controls";
import { text } from "../strings";

function readable(value: unknown): unknown {
  if (value && typeof value === "object" && "intent" in value) {
    const intent = value as { intent: string; value?: unknown };
    return intent.intent === "set"
      ? intent.value
      : intent.intent === "unset"
        ? text("recoveryCompare.empty")
        : text("recoveryCompare.keep");
  }
  return value;
}

function Preview({
  value,
  names,
}: {
  value: unknown;
  names: Map<string, string>;
}) {
  value = readable(value);
  if (value === null || value === undefined)
    return <p>{text("recoveryCompare.absent")}</p>;
  if (typeof value === "string" || typeof value === "number")
    return (
      <p className="recovery-compare-text">
        {typeof value === "string"
          ? (names.get(value) ?? value)
          : String(value)}
      </p>
    );
  if (typeof value === "boolean")
    return <p>{text(value ? "recoveryCompare.yes" : "recoveryCompare.no")}</p>;
  if (typeof value === "object" && "kind" in value) {
    const record = value as Record<string, unknown>;
    if (record.kind === "rich_text" && record.content)
      return <ValueRead value={value as Value} options={[]} />;
    if (record.kind === "group" && Array.isArray(record.instances))
      return <Preview value={record.instances} names={names} />;
    if (record.kind === "unset") return <p>{text("recoveryCompare.empty")}</p>;
    if (record.kind === "number_unknown")
      return <p>{text("field.numberUnknown")}</p>;
    if ("value" in record)
      return <Preview value={record.value} names={names} />;
    if ("option" in record)
      return <Preview value={record.option} names={names} />;
    if ("milliseconds" in record)
      return <Preview value={record.milliseconds} names={names} />;
  }
  if (Array.isArray(value))
    return (
      <ul>
        {value.map((item, index) => (
          <li key={index}>
            <Preview value={item} names={names} />
          </li>
        ))}
      </ul>
    );
  if (typeof value === "object")
    return (
      <dl>
        {Object.entries(value)
          .filter(
            ([key]) =>
              ![
                "id",
                "field",
                "labels",
                "source",
                "lineage",
                "protected",
                "composing",
                "restore",
                "archiveIndex",
                "archiveOrder",
                "archiveTitle",
              ].includes(key),
          )
          .map(([key, item]) => (
            <div key={key}>
              <dt>{names.get(key) ?? propertyLabel(key)}</dt>
              <dd>
                <Preview value={item} names={names} />
              </dd>
            </div>
          ))}
      </dl>
    );
  return null;
}

function propertyLabel(key: string): string {
  const keys: Record<string, string> = {
    name: "이름",
    label: "항목 이름",
    required: "필수 입력",
    writingGuide: "작성 안내",
    presentation: "표시 서식",
    default: "현재 기본값",
    fields: "필드",
    members: "하위 필드",
    options: "선택지",
    sections: "섹션",
    cardTitleField: "카드 제목 필드",
    archived: "보관 상태",
    configuration: "구성",
    kind: "형식",
    minimum: "최솟값",
    maximum: "최댓값",
    $order: "순서",
    englishName: "영문 이름",
    glossarySummary: "한 줄 설명",
    glossaryExcluded: "글로서리 제외",
    value: "내용",
  };
  return keys[key] ?? key;
}

function changeLabel(
  change: RecoveryChange,
  names: Map<string, string>,
): string {
  const identities = change.path.filter((key) =>
    /^[a-z0-9:-]{20,}$/i.test(key),
  );
  const labels = identities.map((id) => names.get(id)).filter(Boolean);
  const property = change.path[change.path.length - 1] ?? "";
  return [...labels, names.get(property) ?? propertyLabel(property)].join(
    " · ",
  );
}

export function RecoveryComparison({
  content,
  selected,
  change,
}: {
  content: RecoveryContent;
  selected: string[];
  change: (ids: string[]) => void;
}) {
  const names = new Map<string, string>();
  const collect = (value: unknown) => {
    if (!value || typeof value !== "object") return;
    if (Array.isArray(value)) {
      value.forEach(collect);
      return;
    }
    const object = value as Record<string, unknown>;
    if (typeof object.id === "string" && typeof object.label === "string")
      names.set(object.id, object.label);
    Object.values(object).forEach(collect);
  };
  collect(content.original);
  collect(content.current);
  collect(content.draft);
  collect(content.comparison?.map((item) => item.preserved));
  return (
    <section aria-label={text("recoveryCompare.title")}>
      <p>{text("recoveryCompare.help")}</p>
      {content.comparison?.map((item) => {
        const chosen = selected.includes(item.id);
        return (
          <section className="recovery-compare-item" key={item.id}>
            <Checkbox
              label={
                changeLabel(item, names) +
                (item.status === "conflict"
                  ? " · " + text("recoveryCompare.conflict")
                  : "")
              }
              disabled={item.status === "blocked"}
              checked={chosen}
              onChange={(_, data) =>
                change(
                  data.checked === true
                    ? [...selected, item.id]
                    : selected.filter((id) => id !== item.id),
                )
              }
            />
            {item.reason && <p role="alert">{item.reason}</p>}
            <div className="recovery-comparison">
              <section>
                <h4>{text("whole.current")}</h4>
                <Preview value={item.current} names={names} />
              </section>
              <section>
                <h4>{text("recoveryCompare.preserved")}</h4>
                <Preview value={item.preserved} names={names} />
              </section>
            </div>
            <p>
              {text(
                chosen
                  ? "recoveryCompare.applyPreserved"
                  : "recoveryCompare.keepCurrent",
              )}
            </p>
          </section>
        );
      })}
      {content.comparison?.length === 0 && (
        <p>{text("recoveryCompare.noChanges")}</p>
      )}
    </section>
  );
}
