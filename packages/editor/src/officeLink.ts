import { createContext } from 'react'

/** Gives the address that opens a file (its name and download address) in the office editor, or `null`. */
export type OfficeEditorHref = (name: string, url: string) => string | null

/**
 * The app provides this around the editor while office editing is on (a feature of the `evolv3ai/orbit` fork).
 * A file block then shows "Open in editor" on the files it gives an address for. Without a provider, no file
 * block shows it.
 */
export const OfficeLinkContext = createContext<OfficeEditorHref | null>(null)

/**
 * Says of a file (its download address) whether its last save from the office editor failed. The app provides
 * it with `OfficeLinkContext`. A file block then shows "Last save failed" next to "Open in editor".
 */
export const OfficeSaveFailedContext = createContext<((url: string) => boolean) | null>(null)

/** What the mark tells on hover, here and on the files of tasks. */
export const SAVE_FAILED_HELP = 'The changes from the last time this file was open in the editor did not reach Orbit. The file is as it was before.'
