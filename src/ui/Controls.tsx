import {
  createContext,
  useContext,
  type ComponentProps,
  type Ref,
} from "react";
import {
  Button as FluentButton,
  Checkbox as FluentCheckbox,
  Input as FluentInput,
  Select as FluentSelect,
  Textarea as FluentTextarea,
  makeStyles,
  mergeClasses,
  shorthands,
  tokens,
  type ButtonProps,
  type CheckboxProps,
  type InputProps,
  type SelectProps,
  type TextareaProps,
} from "@fluentui/react-components";

const DisabledContext = createContext(false);
const useStyles = makeStyles({
  control: { width: "100%", minWidth: 0 },
  button: { maxWidth: "100%", height: "auto", overflowWrap: "anywhere" },
  selected: {
    backgroundColor: tokens.colorBrandBackground2,
    ...shorthands.borderColor(tokens.colorBrandStroke1),
    ":hover": { backgroundColor: tokens.colorBrandBackground2Hover },
    ":active": { backgroundColor: tokens.colorBrandBackground2Pressed },
  },
  multiple: {
    height: "auto",
    minHeight: "96px",
    paddingRight: tokens.spacingHorizontalS,
  },
  danger: {
    color: tokens.colorStatusDangerForeground1,
    ...shorthands.borderColor(tokens.colorStatusDangerBorder1),
    ":hover": { backgroundColor: tokens.colorStatusDangerBackground1 },
    ":active": { backgroundColor: tokens.colorStatusDangerBackground2 },
  },
});

/** native fieldset의 잠금 의미를 Fluent의 시각 상태와 실제 input slot에도 전달한다. */
export function Fieldset({ disabled, ...props }: ComponentProps<"fieldset">) {
  const inherited = useContext(DisabledContext);
  const locked = inherited || !!disabled;
  return (
    <DisabledContext.Provider value={locked}>
      <fieldset {...props} disabled={locked} />
    </DisabledContext.Provider>
  );
}

export function Button({
  danger = false,
  disabled,
  className,
  ...props
}: ButtonProps & { danger?: boolean; ref?: Ref<HTMLButtonElement> }) {
  const inherited = useContext(DisabledContext);
  const styles = useStyles();
  const locked = inherited || !!disabled;
  return (
    <FluentButton
      {...props}
      disabled={locked}
      className={mergeClasses(
        styles.button,
        props["aria-pressed"] === true && !locked && styles.selected,
        danger && !locked && styles.danger,
        className,
      )}
    />
  );
}

export function Input({
  disabled,
  className,
  ...props
}: InputProps & { ref?: Ref<HTMLInputElement> }) {
  const inherited = useContext(DisabledContext);
  const styles = useStyles();
  return (
    <FluentInput
      {...props}
      disabled={inherited || !!disabled}
      className={mergeClasses(styles.control, className)}
    />
  );
}

export function Textarea({
  disabled,
  className,
  ...props
}: TextareaProps & { ref?: Ref<HTMLTextAreaElement> }) {
  const inherited = useContext(DisabledContext);
  const styles = useStyles();
  return (
    <FluentTextarea
      resize="vertical"
      {...props}
      disabled={inherited || !!disabled}
      className={mergeClasses(styles.control, className)}
    />
  );
}

export function Select({ disabled, className, ...props }: SelectProps) {
  const inherited = useContext(DisabledContext);
  const styles = useStyles();
  return (
    <FluentSelect
      {...props}
      icon={props.multiple ? null : undefined}
      select={props.multiple ? { className: styles.multiple } : undefined}
      disabled={inherited || !!disabled}
      className={mergeClasses(styles.control, className)}
    />
  );
}

export function Checkbox({ disabled, ...props }: CheckboxProps) {
  const inherited = useContext(DisabledContext);
  return <FluentCheckbox {...props} disabled={inherited || !!disabled} />;
}
