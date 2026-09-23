// Qlisa brand mark. The component name stays for internal compatibility.
import { QLISA_LOGO_ALT, QLISA_LOGO_SRC } from "../../lib/brandAssets";

export function InkueMark({ size = 20 }: { size?: number }) {
  return (
    <img
      src={QLISA_LOGO_SRC}
      alt={QLISA_LOGO_ALT}
      width={size}
      height={size}
      draggable={false}
      style={{ display: "block" }}
    />
  );
}
