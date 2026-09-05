import { page } from '$app/state'

// the form remembers the last token and chains; a shared link can preset them
// with ?token=&from=&to=
export function formDefault(
  name: 'Token' | 'From' | 'To',
  fallback: string
): string {
  return (
    page.url.searchParams.get(name.toLowerCase()) ||
    localStorage.getItem('default' + name) ||
    fallback
  )
}

export function rememberForm(token: string, from: string, to: string) {
  localStorage.setItem('defaultToken', token)
  localStorage.setItem('defaultFrom', from)
  localStorage.setItem('defaultTo', to)
}
