import {
  Toolbar,
  ToolbarButton,
  ToolbarDivider,
  Tooltip,
} from "@fluentui/react-components";
import {
  TextBold20Regular,
  TextItalic20Regular,
  TextUnderline20Regular,
  TextStrikethrough20Regular,
  TextParagraph20Regular,
  TextHeader320Regular,
  TextQuote20Regular,
  TextBulletListLtr20Regular,
  TextNumberListLtr20Regular,
  TaskListLtr20Regular,
  CheckboxChecked20Regular,
  ArrowUndo20Regular,
  ArrowRedo20Regular,
} from "@fluentui/react-icons";
import { text } from "../../strings";

const icons = {
  bold: TextBold20Regular,
  italic: TextItalic20Regular,
  underline: TextUnderline20Regular,
  strikethrough: TextStrikethrough20Regular,
  paragraph: TextParagraph20Regular,
  heading: TextHeader320Regular,
  blockquote: TextQuote20Regular,
  bulletList: TextBulletListLtr20Regular,
  orderedList: TextNumberListLtr20Regular,
  taskList: TaskListLtr20Regular,
  toggleTask: CheckboxChecked20Regular,
  undo: ArrowUndo20Regular,
  redo: ArrowRedo20Regular,
};
export type RichAction = keyof typeof icons;
const shortcuts: Partial<Record<RichAction, { label: string; aria: string }>> =
  {
    bold: { label: "Ctrl+B", aria: "Control+b" },
    italic: { label: "Ctrl+I", aria: "Control+i" },
    underline: { label: "Ctrl+U", aria: "Control+u" },
    undo: { label: "Ctrl+Z", aria: "Control+z" },
    redo: { label: "Ctrl+Y / Ctrl+Shift+Z", aria: "Control+y Control+Shift+z" },
  };

/** 아이콘의 모양과 별개로 이름·툴팁·눌림 상태를 접근성 트리에도 제공한다. */
export function RichToolbar({
  disabled,
  selected,
  taskAvailable,
  action,
}: {
  disabled: boolean;
  selected: readonly string[];
  taskAvailable: boolean;
  action: (kind: RichAction) => void;
}) {
  return (
    <Toolbar
      className="rich-toolbar"
      aria-label={text("rich.toolbar")}
      onMouseDown={(event) => event.preventDefault()}
    >
      {(Object.keys(icons) as RichAction[]).map((kind) => {
        const Icon = icons[kind];
        const label = text(`rich.${kind}`);
        const shortcut = shortcuts[kind];
        return (
          <span className="rich-tool" key={kind}>
            {(kind === "paragraph" || kind === "undo") && <ToolbarDivider />}
            <Tooltip
              content={shortcut ? `${label} (${shortcut.label})` : label}
              relationship="description"
              showDelay={2000}
            >
              <ToolbarButton
                type="button"
                aria-label={label}
                aria-keyshortcuts={shortcut?.aria}
                icon={<Icon />}
                disabled={disabled || (kind === "toggleTask" && !taskAvailable)}
                aria-pressed={
                  kind === "undo" || kind === "redo"
                    ? undefined
                    : selected.includes(kind)
                }
                onClick={() => action(kind)}
              />
            </Tooltip>
          </span>
        );
      })}
    </Toolbar>
  );
}
