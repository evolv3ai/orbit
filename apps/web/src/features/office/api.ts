import { useQuery } from '@tanstack/react-query'
import { getOfficeStatus } from '@orbit/api/generated/sdk.gen'
import { apiClient } from '@/api/client'
import { queryKeys } from '@/api/queryKeys'
import { officeEditorHref } from '@/features/office/officeFiles'

/** True when this Orbit opens office files in the editor. False until the server has answered. */
export function useOfficeEnabled() {
  const status = useQuery({
    queryKey: queryKeys.officeStatus,
    queryFn: async () => (await getOfficeStatus({ client: apiClient, throwOnError: true })).data,
    // An admin changes this rarely.
    staleTime: 5 * 60_000,
  })
  return status.data?.enabled ?? false
}

/**
 * Gives the address of "Open in editor" for a file (its name and download address), or `null` for a file that
 * has no such action. `undefined` while office editing is off: then no file has the action.
 */
export function useOfficeEditorHref() {
  return useOfficeEnabled() ? officeEditorHref : undefined
}
