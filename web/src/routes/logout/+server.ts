import { redirect } from '@sveltejs/kit';
import type { RequestHandler } from './$types';

export const POST: RequestHandler = async ({ cookies }) => {
  cookies.delete('sp_token', { path: '/' });
  cookies.delete('sp_tenant', { path: '/' });
  throw redirect(303, '/login');
};
