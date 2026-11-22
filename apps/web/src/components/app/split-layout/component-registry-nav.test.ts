import * as routes from '@app/routes/routes';
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

// A pane opened on a `view-<name>` route renders the component `<name>`.
const viewRouteIds: string[] = [];
for (const route of Object.values(routes)) {
  if (typeof route !== 'object' || route === null || !('id' in route)) continue;
  if (typeof route.id === 'string' && route.id.startsWith('view-'))
    viewRouteIds.push(route.id);
}

describe('the view routes', () => {
  it.each(viewRouteIds)('open a registered component: %s', (id) => {
    expect(() => resolveComponent(id.slice('view-'.length))).not.toThrow();
  });
});
