import { listDocs } from '$lib/server/docs';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = () => {
  const all = listDocs();
  return {
    reference: all.filter((d) => d.category === 'reference'),
    examples: all.filter((d) => d.category === 'example'),
  };
};
