const paths = {
  panel: <path d="M3 4h18v16H3ZM8 4v16" />,
  editor: <path d="M3 4h18v16H3ZM3 11h18m-13 3v3m4-3v3m4-3v3" />,
  browser: <path d="M3 4h18v16H3Zm13 0v16m-9-12h5m-5 4h5" />,
  folder: <path d="M3 6h7l2 2h9v12H3Z" />,
  search: <path d="m16 16 5 5M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0" />,
  note: <path d="M10 17V5l10-2v12M10 5v4l10-2M10 17c0 4-7 4-7 1s7-4 7-1m10-2c0 4-7 4-7 1s7-4 7-1" />,
  wave: <path d="M2 12h3l2-7 4 14 3-17 3 13 2-3h3" />,
  power: <path d="M12 2v10m6.4-6.4a9 9 0 1 1-12.8 0" />,
  play: <path d="m8 5 11 7-11 7Z" />,
  pause: <path d="M8 5v14m8-14v14" />,
  stop: <path d="M6 6h12v12H6Z" />,
  rewind: <path d="m11 6-7 6 7 6Zm9 0-7 6 7 6Z" />,
  loop: <path d="M17 2l4 4-4 4M3 11V9a3 3 0 0 1 3-3h15m-14 16-4-4 4-4m14-1v2a3 3 0 0 1-3 3H3" />,
  sliders: <path d="M4 7h10m4 0h2M4 17h2m4 0h10M14 4v6M10 14v6" />,
};

export function Icon({ name, size = 18 }) {
  return (
    <svg className="icon" width={size} height={size} viewBox="0 0 24 24" fill="none"
      stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round"
      aria-hidden="true" focusable="false">
      {paths[name]}
    </svg>
  );
}
