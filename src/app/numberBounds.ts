/** 숫자 본문은 부동소수점으로 바꾸지 않는다. 부호·정수 길이·소수 자릿수로 비교한다. */
export function decimalCompare(a: string, b: string): number | null {
  const pattern = /^-?(?:0|[1-9]\d*)(?:\.\d*[1-9])?$/;
  if (a === "-0" || b === "-0" || !pattern.test(a) || !pattern.test(b))
    return null;
  const parts = (raw: string) => {
    const [whole, fraction = ""] = raw.replace(/^-/, "").split(".");
    return {
      negative: raw.startsWith("-") && /[1-9]/.test(whole + fraction),
      whole,
      fraction,
    };
  };
  const x = parts(a),
    y = parts(b);
  if (x.negative !== y.negative) return x.negative ? -1 : 1;
  const width = Math.max(x.fraction.length, y.fraction.length);
  const left = x.whole + x.fraction.padEnd(width, "0"),
    right = y.whole + y.fraction.padEnd(width, "0");
  const result =
    x.whole.length !== y.whole.length
      ? Math.sign(x.whole.length - y.whole.length)
      : left === right
        ? 0
        : left < right
          ? -1
          : 1;
  return x.negative ? -result : result;
}
export function numberInBounds(
  value: string,
  minimum?: string | null,
  maximum?: string | null,
): boolean {
  if (decimalCompare(value, value) === null) return false;
  return (
    (!minimum || (decimalCompare(value, minimum) ?? -1) >= 0) &&
    (!maximum || (decimalCompare(value, maximum) ?? 1) <= 0)
  );
}
export function boundsProblem(
  minimum?: string | null,
  maximum?: string | null,
): "minimum" | "maximum" | null {
  if (minimum && decimalCompare(minimum, minimum) === null) return "minimum";
  if (maximum && decimalCompare(maximum, maximum) === null) return "maximum";
  return minimum && maximum && (decimalCompare(minimum, maximum) ?? 1) > 0
    ? "maximum"
    : null;
}
