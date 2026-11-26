import { definePlugin } from '@decky/api';
import { staticClasses } from '@decky/ui';
import { Panel } from './Panel';

function Icon() {
  return (
    <svg width="1em" height="1em" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"
      strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M4 10v4" />
      <path d="M8 7v10" />
      <path d="M12 4v16" />
      <path d="M16 7v10" />
      <path d="M20 10v4" />
    </svg>
  );
}

export default definePlugin(() => ({
  name: 'Audio Bridge',
  titleView: <div className={staticClasses.Title}>Audio Bridge</div>,
  content: <Panel />,
  icon: <Icon />,
}));
