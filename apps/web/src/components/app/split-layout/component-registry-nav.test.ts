import { sidebarContent } from '@components/app/app-sidebar/sidebar';
import { SIDEBAR_NEXT_NAV_ITEMS } from '@components/app/sidebar-next/nav-items';
import { describe, expect, it } from 'vitest';
import { resolveComponent } from './componentRegistry';

describe('the sidebar items', () => {
  it.each(SIDEBAR_NEXT_NAV_ITEMS.map((item) => item.id))(
    'open a registered component: %s',
    (id) => {
      expect(() => resolveComponent(sidebarContent(id).id)).not.toThrow();
    }
  );
});
