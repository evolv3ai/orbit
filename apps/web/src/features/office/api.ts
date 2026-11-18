import { useCallback, useMemo } from 'react'
import { useQuery } from '@tanstack/react-query'
import { getOfficeStatus, listOfficeSaveFailures } from '@orbit/api/generated/sdk.gen'
import { apiClient } from '@/api/client'
import { queryKeys } from '@/api/queryKeys'
import { officeEditorHref, officeFileOfUrl } from '@/features/office/officeFiles'

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

/**
 * Says of a file (its download address) whether its last save from the office editor failed: the file in Orbit
 * does not have the changes of that editing session. `undefined` while office editing is off.
 */
export function useOfficeSaveFailed() {
  const enabled = useOfficeEnabled()
  const failures = useQuery({
    queryKey: queryKeys.officeSaveFailures,
    queryFn: async () => (await listOfficeSaveFailures({ client: apiClient, throwOnError: true })).data,
    enabled,
    // A person comes back from the editor's tab: the focus asks again.
    staleTime: 30_000,
  })
  const failed = useMemo(() => new Set((failures.data?.files ?? []).map((file) => `${file.kind}:${file.file_id}`)), [failures.data])
  const saveFailed = useCallback(
    (url: string) => {
      const file = officeFileOfUrl(url)
      return file !== null && failed.has(`${file.kind}:${file.id}`)
    },
    [failed],
  )
  return enabled ? saveFailed : undefined
}
