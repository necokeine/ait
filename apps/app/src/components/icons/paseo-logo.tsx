import Svg, { Circle, Path, Rect } from "react-native-svg";

interface PaseoLogoProps {
  size?: number;
  color?: string;
}

// Keep the geometry and default colors aligned with the desktop icon in /logo.svg.
export function PaseoLogo({ size = 64, color }: PaseoLogoProps) {
  return (
    <Svg width={size} height={size} viewBox="0 0 1024 1024" fill="none">
      {color === undefined && (
        <Rect x={64} y={64} width={896} height={896} rx={208} fill="#151613" />
      )}
      <Path
        d="M280 728 484 296Q512 240 540 296L744 728"
        stroke={color ?? "#b8ef73"}
        strokeWidth={88}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <Path d="M392 568H632" stroke={color ?? "#b8ef73"} strokeWidth={72} strokeLinecap="round" />
      <Circle cx={512} cy={568} r={52} fill={color ?? "#eceee8"} />
    </Svg>
  );
}
