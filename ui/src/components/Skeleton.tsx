import type { CSSProperties } from "react";

interface Props {
  width?: number | string;
  height?: number | string;
  /** Vertical spacing below this skeleton block. */
  mb?: number;
  style?: CSSProperties;
}

/** Shimmering placeholder used while data loads. */
export function Skeleton({ width = "100%", height = 12, mb, style }: Props) {
  return (
    <span
      className="skeleton"
      style={{
        width,
        height,
        display: "block",
        marginBottom: mb,
        ...style,
      }}
    />
  );
}

/** Render N skeleton table rows so the docs list reserves layout while
 *  list_documents is in flight. */
export function SkeletonRows({ rows = 6, columns = 4 }: { rows?: number; columns?: number }) {
  return (
    <tbody>
      {Array.from({ length: rows }).map((_, ri) => (
        <tr key={ri}>
          {Array.from({ length: columns }).map((_, ci) => (
            <td key={ci}>
              <Skeleton width={ci === 0 ? "60%" : "30%"} />
            </td>
          ))}
        </tr>
      ))}
    </tbody>
  );
}
