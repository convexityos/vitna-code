/**
 * The window's few marks, drawn to Vitna's terms for them: a 16-unit box,
 * a 1.7 stroke in currentColor, butt caps, miter joins, no fill. Each sits
 * beside a word or carries an accessible label; none is the only way to know
 * what a control does.
 */

import type { SVGProps } from 'react';

type IconProps = Omit<SVGProps<SVGSVGElement>, 'children'>;

function Mark({ d, ...props }: IconProps & { d: string }) {
  return (
    <svg
      className="icon"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.7}
      strokeLinecap="butt"
      strokeLinejoin="miter"
      aria-hidden="true"
      focusable="false"
      {...props}
    >
      <path d={d} />
    </svg>
  );
}

export const MenuIcon = (p: IconProps) => <Mark d="M2.5 4.5h11M2.5 8h11M2.5 11.5h11" {...p} />;
export const PlusIcon = (p: IconProps) => <Mark d="M8 2.5v11M2.5 8h11" {...p} />;
export const SendIcon = (p: IconProps) => <Mark d="M8 13.5V3M3.5 7.5 8 3l4.5 4.5" {...p} />;
export const ChevronDownIcon = (p: IconProps) => <Mark d="m4 6 4 4 4-4" {...p} />;
export const ChevronRightIcon = (p: IconProps) => <Mark d="m6 4 4 4-4 4" {...p} />;
export const CloseIcon = (p: IconProps) => <Mark d="m3.5 3.5 9 9M12.5 3.5l-9 9" {...p} />;
export const CheckIcon = (p: IconProps) => <Mark d="m3 8.5 3.2 3.2L13 4.8" {...p} />;
export const SearchIcon = (p: IconProps) => <Mark d="M7 12a5 5 0 1 0 0-10 5 5 0 0 0 0 10ZM10.6 10.6 14 14" {...p} />;
export const NewSessionIcon = (p: IconProps) => (
  <Mark d="M13.5 8.5V13.5H2.5V2.5H7.5M11.2 2.3l2.5 2.5L8.2 10.3H5.7V7.8Z" {...p} />
);
export const ChangesIcon = (p: IconProps) => <Mark d="M3 4.5h6M6 1.5v6M3 12h6M10.5 2.5h3v11h-3" {...p} />;
export const PauseIcon = (p: IconProps) => <Mark d="M5.5 3v10M10.5 3v10" {...p} />;
export const PlayIcon = (p: IconProps) => <Mark d="M4.5 2.8v10.4L12.8 8Z" {...p} />;
export const StopIcon = (p: IconProps) => <Mark d="M3.5 3.5h9v9h-9Z" {...p} />;
export const FolderIcon = (p: IconProps) => <Mark d="M1.8 3.2h4.4l1.5 1.6h6.5v8H1.8Z" {...p} />;
