import type { Template } from "../bridge/types";
/** 과거 표현 token은 원문 호환을 위해 보존하지만 화면에는 항상 기본 표현만 사용한다. */
export function presentationClass(
  _template?: string | null,
  _field?: string | null,
) {
  return "presentation-standard";
}
export function SectionTitles({
  template,
  before,
}: {
  template: Template;
  before: string | null;
}) {
  return (
    <>
      {template.sections
        ?.filter((s) => s.beforeField === before)
        .map((s) => (
          <h2 key={s.id} className="document-section">
            {s.title}
          </h2>
        ))}
    </>
  );
}
