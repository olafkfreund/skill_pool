import { error } from '@sveltejs/kit';
import { loadDoc } from '$lib/server/docs';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = ({ params }) => {
  const doc = loadDoc(params.slug);
  if (!doc) {
    throw error(404, `No doc named "${params.slug}".`);
  }
  return { doc };
};
