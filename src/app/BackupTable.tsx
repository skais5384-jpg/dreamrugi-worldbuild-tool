import type { ReactNode } from "react";
import { Button } from "../ui/Controls";

export interface BackupDisplayRow {
  id: string;
  name: string;
  state: ReactNode;
  date: ReactNode;
  size: string;
  disabled: boolean;
}

/** Comparison columns remain visible while the modal body owns scrolling. */
export function BackupTable({
  rows,
  selected,
  select,
  dateLabel = "생성 일시",
}: {
  rows: BackupDisplayRow[];
  selected?: string | null;
  select: (id: string) => void;
  dateLabel?: string;
}) {
  if (!rows.length) return null;
  return (
    <table className="backup-table">
      <thead>
        <tr>
          <th scope="col">이름</th>
          <th scope="col">상태 / 종류</th>
          <th scope="col">{dateLabel}</th>
          <th scope="col">크기</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <tr key={row.id} data-selected={selected === row.id}>
            <td>
              <Button
                type="button"
                appearance="subtle"
                className="backup-select"
                aria-pressed={selected === row.id}
                disabled={row.disabled}
                onClick={() => select(row.id)}
              >
                {row.name}
              </Button>
            </td>
            <td>{row.state}</td>
            <td>{row.date}</td>
            <td>{row.size}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
