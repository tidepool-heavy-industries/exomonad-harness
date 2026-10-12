import type { ReactNode } from 'react'

export function Empty({ children }: {
  children: ReactNode;
}) {
  return <p className="empty" role="status">
    {children}
  </p>;
}
export function Pager({ page, count, total, onPage }: {
  page: number;
  count: number;
  total: number;
  onPage: (page: number) => void;
}) {
  return <div className="pager">
    <span>
      {total} rows · Page {page + 1} of {Math.max(1, count)}
    </span>
    <button disabled={page === 0} onClick={() => onPage(page - 1)}>Previous page</button>
    <button disabled={page + 1 >= count} onClick={() => onPage(page + 1)}>Next page</button>
  </div>;
}
