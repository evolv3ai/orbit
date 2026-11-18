import type { ReactNode } from 'react'
import { OfficeLinkContext, OfficeSaveFailedContext } from '@orbit/editor/officeLink'
import { useOfficeEditorHref, useOfficeSaveFailed } from '@/features/office/api'

/**
 * Gives the editors below it "Open in editor" on office files, and "Last save failed" on the ones whose last
 * save from the editor failed, while office editing is on.
 */
export function OfficeLinks({ children }: { children: ReactNode }) {
  return (
    <OfficeLinkContext value={useOfficeEditorHref() ?? null}>
      <OfficeSaveFailedContext value={useOfficeSaveFailed() ?? null}>{children}</OfficeSaveFailedContext>
    </OfficeLinkContext>
  )
}
