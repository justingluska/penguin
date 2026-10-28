// An extension's icon: its own SVG (a data: URL, drawn by an <img> so it
// can't run anything), else a built-in icon it named, else a generic one.
import { Icon, isIconName, type IconSize } from "../../components/Icon";

export function ExtensionIcon({ icon, svg, size = "xs" }: { icon: string | null; svg: string | null; size?: IconSize }) {
  if (svg) return <img className={`ext-icon i i-${size}`} src={svg} alt="" aria-hidden="true" draggable={false} />;
  return <Icon name={isIconName(icon) ? icon : "zap"} size={size} />;
}
