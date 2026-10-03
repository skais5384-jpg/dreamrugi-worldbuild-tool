import type { RecoveryContent } from "../bridge/workspace";

// Reading/copying preserved input is independent of project write authority.
// URLs and attachment references are plain text; this renderer loads no media.
export function recoveryText(content: RecoveryContent): string {
  const names = new Map<string, string>();
  const collect = (value: unknown) => {
    if (!value || typeof value !== "object") return;
    if (Array.isArray(value)) {
      value.forEach(collect);
      return;
    }
    const record = value as Record<string, unknown>;
    if (typeof record.id === "string" && typeof record.label === "string")
      names.set(record.id, record.label);
    Object.values(record).forEach(collect);
  };
  collect(content.original);
  collect(content.current);
  collect(content.draft);
  const labels: Record<string, string> = {
    name: "이름",
    englishName: "영문 이름",
    glossarySummary: "한 줄 설명",
    glossaryExcluded: "글로서리 제외",
    label: "이름",
    fields: "필드",
    members: "하위 필드",
    options: "선택지",
    required: "필수",
    archived: "보관",
    default: "기본값",
    configuration: "구성",
    cardTitleField: "카드 제목 필드",
    sections: "섹션",
    title: "제목",
    beforeField: "표시 위치",
    value: "내용",
    instances: "반복 카드",
    content: "본문",
    links: "연결",
    documents: "연결 문서",
    option: "선택",
    milliseconds: "기간",
    minimum: "최솟값",
    maximum: "최댓값",
    presentation: "표시 서식",
    writingGuide: "작성 안내",
    edit: "템플릿 변경",
    edits: "문서 변경",
    field: "필드",
    insertion: "표시 위치",
  };
  const format = (value: unknown, depth = 0): string => {
    if (value === null || value === undefined) return "없음";
    if (typeof value === "string") return names.get(value) ?? value;
    if (typeof value === "boolean") return value ? "예" : "아니오";
    if (typeof value !== "object") return String(value);
    if (Array.isArray(value))
      return value
        .map((item, i) => `${i + 1}. ${format(item, depth + 1)}`)
        .join("\n");
    const record = value as Record<string, unknown>;
    if (record.intent === "keep") return "현재 값 유지";
    if (record.intent === "unset") return "미설정";
    if (record.intent === "set") return format(record.value, depth);
    if (record.kind === "text") return String(record.text ?? "");
    if (Array.isArray(record.children))
      return record.children
        .map((child) => format(child, depth))
        .join(record.kind === "root" ? "\n" : "");
    const field =
      typeof record.field === "string"
        ? (names.get(record.field) ?? record.field)
        : "";
    return Object.entries(record)
      .filter(
        ([key]) =>
          ![
            "id",
            "field",
            "source",
            "lineage",
            "protected",
            "labels",
            "composing",
            "restore",
            "archiveIndex",
            "archiveOrder",
            "archiveTitle",
            "kind",
            "marks",
            "template",
            "document",
            "revision",
          ].includes(key),
      )
      .map(
        ([key, item]) =>
          `${"  ".repeat(depth)}${field || labels[key] || key}: ${format(item, depth + 1)}`,
      )
      .join("\n");
  };
  return format(content.draft);
}
