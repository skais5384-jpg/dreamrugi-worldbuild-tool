import "./EmptyState.css";

export function EmptyState({ children }: { children: React.ReactNode }) {
  return (
    <div className="content-empty-state">
      <img
        src="/brand/dreamrugi-info-cat.png"
        width="125"
        height="125"
        alt=""
        aria-hidden="true"
      />
      <p>{children}</p>
    </div>
  );
}
