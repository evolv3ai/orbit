import type { ReactNode } from 'react'
import { OfficeLinkContext } from '@orbit/editor/officeLink'
import { useOfficeEditorHref } from '@/features/office/api'

/** Gives the editors below it "Open in editor" on office files, while office editing is on. */
export function OfficeLinks({ children }: { children: ReactNode }) {
  return <OfficeLinkContext value={useOfficeEditorHref() ?? null}>{children}</OfficeLinkContext>
}
