import {
  mdiAlertCircleOutline,
  mdiCheck,
  mdiClose,
  mdiCodeJson,
  mdiDeleteOutline,
  mdiExportVariant,
  mdiFilterVariant,
  mdiFormatText,
  mdiImageOutline,
  mdiPause,
  mdiPlay,
  mdiPlus,
  mdiRedo,
  mdiShapeOutline,
  mdiSkipPrevious,
  mdiStepBackward,
  mdiStepForward,
  mdiTuneVariant,
  mdiUndo,
  mdiArrowExpandRight,
  mdiKeyboardOutline,
  mdiCheckCircleOutline,
  mdiVideoOutline,
  mdiFolderOpenOutline,
  mdiContentSaveOutline,
  mdiRadar,
} from '@mdi/js';

/** MDI 图标。路径来自 @mdi/js，不依赖网络字体。 */
const PATHS = {
  play: mdiPlay,
  pause: mdiPause,
  skipPrevious: mdiSkipPrevious,
  stepBackward: mdiStepBackward,
  stepForward: mdiStepForward,
  plus: mdiPlus,
  export: mdiExportVariant,
  check: mdiCheck,
  close: mdiClose,
  delete: mdiDeleteOutline,
  filter: mdiFilterVariant,
  tune: mdiTuneVariant,
  undo: mdiUndo,
  redo: mdiRedo,
  alert: mdiAlertCircleOutline,
  text: mdiFormatText,
  image: mdiImageOutline,
  region: mdiShapeOutline,
  extend: mdiArrowExpandRight,
  keyboard: mdiKeyboardOutline,
  done: mdiCheckCircleOutline,
  json: mdiCodeJson,
  videoOpen: mdiVideoOutline,
  folderOpen: mdiFolderOpenOutline,
  save: mdiContentSaveOutline,
  scan: mdiRadar,
} as const;

export type IconName = keyof typeof PATHS;

interface Props {
  name: IconName;
  size?: number;
  className?: string;
}

export function Icon({ name, size = 18, className }: Props) {
  return (
    <svg className={`mdi ${className ?? ''}`} width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <path d={PATHS[name]} fill="currentColor" />
    </svg>
  );
}
