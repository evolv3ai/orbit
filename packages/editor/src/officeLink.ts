import { createContext } from 'react'

/** Gives the address that opens a file (its name and download address) in the office editor, or `null`. */
export type OfficeEditorHref = (name: string, url: string) => string | null

/**
 * The app provides this around the editor while office editing is on (a feature of the `evolv3ai/orbit` fork).
 * A file block then shows "Open in editor" on the files it gives an address for. Without a provider, no file
 * block shows it.
 */
export const OfficeLinkContext = createContext<OfficeEditorHref | null>(null)
