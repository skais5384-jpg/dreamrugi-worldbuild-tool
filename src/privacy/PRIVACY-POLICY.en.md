# Dreamrugi Worldbuild Tool Privacy Policy

- Operator: Dreamrugi — an individual operating the app free of charge from South Korea
- Product: the GitHub and Microsoft Store editions of Dreamrugi Worldbuild Tool 1.0.0
- Privacy and support contact: [skais5384@naver.com](mailto:skais5384@naver.com)
- Effective date: October 1, 2026
- Privacy contact: Dreamrugi, skais5384@naver.com
- Public policy: Privacy and YouTube settings in app Help, and the privacy policy documents in the GitHub repository

## 1. Scope and basic design

Dreamrugi Worldbuild Tool is a Windows desktop app for writing and managing documents and worldbuilding materials. This policy covers information processing by the app and support or privacy inquiries received by the operator. The blog service's own policy governs its visitor records and accounts, including visits to the page hosting this policy.

Documents and attachments are stored on your device by default. The app does not require its own account and has no feature that automatically uploads your projects to a Dreamrugi server, serves the app's own advertising, analyzes app usage, or automatically sends error reports to the operator. External content, updates, Windows and WebView2 services, and collaboration servers you configure can involve separate network connections.

Locally processed documents and files may contain personal information. This does not mean that no personal information is processed or that the app always operates without external connections.

## 2. Information stored or processed on your device

| Information | Purpose and location | Retention and deletion |
| --- | --- | --- |
| Document titles, text, relationships, templates, attachments, and file paths | Creating, opening, and saving materials in your chosen project folder | Ordinary project materials have no general automatic expiry period. You manage the materials and their copies. |
| Project backups | Backup and restoration in your chosen backup folder | Ordinary backups do not expire automatically. Deleted backups are retained for recovery for 7 days. Cleanup is attempted during relevant listing or further deletion operations after expiry, so inactivity or cleanup failures may leave them for longer. Permanent deletion is also available. |
| Retained input, original snapshots, attachment copies, and project fingerprints | Preventing input loss, recovering interrupted work, and checking conflicts; local app data or a separate recovery folder | There is no general time-based expiry. Discarding a recovery item removes only part of that item's records; it does not erase every generation, original snapshot, or attachment copy. |
| YouTube display choice, policy and processing-scope version, and choice time | Remembering your in-app YouTube loading choice in the app user's WebView storage on this device | Updated when you allow, refuse, or withdraw permission. Not included in projects or SVN shared data. Players are not loaded if the setting cannot be read or is not valid for the current version. |
| Settings, display and work state, and project and backup locations | Restoring your working environment; local app data and WebView storage | Managed in the relevant storage locations. Uninstalling the app does not guarantee deletion of all related materials. |
| Activity records and diagnostic information | Diagnosing errors and progress; app memory and local diagnostic files | On-screen records are limited by count; disk diagnostic files are rotated by size and file count. This is not comprehensive age-based deletion. You manage any diagnostic files you export. |
| Words and results for spelling checks | Checking spelling with the bundled local checker and dictionary; temporary files | Removal is attempted after completion. Crashes or errors can leave files behind. The spelling feature does not send the words to a spelling-check server. |
| Documents, local images, fonts, and output files used for PDF export | Creating PDFs with a local renderer; temporary output folders and your chosen destination | Temporary cleanup is attempted after completion or cancellation. Some temporary files or renderer storage may remain. PDFs you save are not automatically deleted. |
| Copies of attachments opened in associated programs | Opening files in external Windows programs; the device's temporary folder | Cleanup of older copies is attempted during subsequent opening operations. Closing the external program does not immediately delete the copy. |

Diagnostic records can include timestamps, processing stages, error categories, results, operation identifiers, and project fingerprints. Recovery and interrupted-save records can include original input or paths. The limited fields used in diagnostic logs do not describe every retained file.

PDF export uses the saved document, local images, and fonts without loading external videos or YouTube players. The underlying WebView2 processing used for export is described in section 3. Further processing by an external program used to open an attachment is subject to that program's policy.

If you place projects or backups in cloud-synchronized or shared folders, your chosen service may synchronize them. Review that service's settings and policies as well.

## 3. When external connections occur

| Feature | Information an external recipient may receive | Timing and recipient |
| --- | --- | --- |
| Update checks in the GitHub distribution | Connection information such as IP address, request time and URL, and the update tool's User-Agent | The app checks for updates automatically on startup and may retry failed checks. It connects to GitHub servers hosting update information. |
| Update downloads in the GitHub distribution | The above connection information and the requested installer | The app connects to GitHub download servers when you choose to update. Update request payloads do not include project text or SVN passwords. |
| Installation and updates in the Microsoft Store distribution | Account, device, installation, and update information processed by Microsoft | Handled by Microsoft Store. This distribution does not use the app's GitHub updater. |
| Windows and WebView2 services | Connection information needed for security checks and required or optional diagnostic and crash information | Processing may occur according to Microsoft's services and your device, account, and diagnostic settings. It is separate from automatic error reporting to Dreamrugi. |
| External images, videos, and YouTube | Requested URLs or video identifiers, IP address, request time, browser and device information, and service-specific cookies or stored information | Connections to the specified content server or Google/YouTube may begin when content appears in a document or editing preview. See section 4. |
| External website and email links | Connection or message information processed by the relevant website or email service | Choosing to open a link externally passes it to your default browser or email program. |
| SVN collaboration | Information needed for the operation, such as server address, authentication account and password, repository paths, locks, files and document text, and commit messages | Sent to the server you specify. Status and lock checks may run automatically after connection setup; commits follow your actions. A local repository differs from transmission to a remote server. |

Choosing “Later” in the GitHub update prompt does not permanently stop automatic update checks on future launches.

Update information is requested from raw.githubusercontent.com and, when needed, release verification information from api.github.com. Choosing retry after a failed check makes another connection. Installer files are downloaded after you choose to update.

Neither edition separately disables WebView2's default Microsoft Defender SmartScreen or diagnostic behavior. Information needed for SmartScreen security checks may be sent to Microsoft. WebView2 processes required diagnostics, optional diagnostics according to Windows settings, and may send crash diagnostic files to Microsoft. You manage optional diagnostics through Windows diagnostic settings. Dreamrugi does not receive this information directly.

External services retain connection records and collaboration history according to their policies or server practices. Deleting a link, setting, or file in the app does not automatically erase records already held by those services. Processing may occur outside your country depending on server locations and service arrangements. A URL alone does not establish a processing country or legal relationship.

## 4. External images, videos, and YouTube

Local attachments and external URL content are handled differently. An external image is retrieved from its server when displayed. A regular video may connect to retrieve basic information, such as its duration, before you press play. A YouTube player may connect when it appears after the policy acceptance and loading permission described below. Network activity can therefore occur before you choose to play content or open it externally.

YouTube starts blocked, with only a URL link displayed. Selecting the link opens an external browser and does not grant permission for in-app loading. In Privacy and YouTube settings in Help, you can separately accept this policy and allow automatic loading of YouTube players and thumbnails. Once allowed, players and thumbnails for displayed videos load automatically; playback starts only when you press play. Your choice is remembered in the same app-user environment on this device, rather than requested for each video or project. You can withdraw permission in the same settings. Withdrawal removes open players, prevents subsequent in-app YouTube loading, and returns to URL links. It does not delete records already sent externally.

Ordinary images display automatically and ordinary videos can retrieve information before playback; these are separate from the YouTube choice. External URLs in projects written by someone else follow the same behavior.

The app restricts the referring-page address sent with image requests and uses a fixed app-origin identifier for YouTube. These measures do not hide your IP address or block all cookies, tracking, or external connections. YouTube uses the Privacy Enhanced Mode embed address. This mode does not eliminate Google/YouTube connections or all cookies, advertising, or storage processing.

Use of YouTube content is subject to the [YouTube Terms of Service](https://www.youtube.com/t/terms). Google's processing is described in the [Google Privacy Policy](https://policies.google.com/privacy). Depending on the service and environment, external players may show advertising or use cookies and similar storage technologies. This differs from the app having its own advertising SDK or analytics feature.

Avoid using content URLs that contain sensitive information or private access tokens. Consult the relevant service's guidance for managing sign-in state, cookies, device storage, and related privacy requests.

## 5. Support and privacy inquiries

When you email the operator, the operator receives your sender address, display name, message, and any files you choose to attach. These are used as needed to respond, resolve the issue you requested help with, or handle a privacy rights request, and are not used for advertising or promotion. The app does not automatically email diagnostics or projects to the support address.

The operator uses NAVER Mail. Your sending email service and receiving services such as NAVER also participate in transmitting and storing messages. The operator's responsibility for handling inquiries is distinct from each email provider's own processing.

Inquiry materials are kept as needed to resolve the inquiry and **deleted once 90 days have passed after resolution**. Information no longer needed is removed earlier, particularly unnecessary sensitive attachments. Unresolved inquiries are also reviewed for continuing need so that unnecessary materials are not retained indefinitely.

Deletion covers related received and sent mail, copies in Trash, downloaded attachments, and separate working copies under the operator's control. Moving messages to Trash does not complete deletion; permanent deletion is performed. Files saved on the operator's devices are removed using deletion procedures appropriate to their storage. The operator cannot directly and immediately erase email-provider backups or copies held by senders; the relevant services' practices apply to those copies.

If an applicable law actually requires retention of specific information, only that information is retained separately for the required period. A blanket long-term statutory retention period is not applied to every inquiry.

Please do not send passwords, authentication tokens, or unnecessary information about other people. When materials are needed, send the relevant portion and obscure unnecessary details. Sending an email is not treated as blanket consent to all uses, disclosures, or international transfers.

## 6. Security and deletion considerations

External media URLs are restricted to HTTPS and some address formats are rejected. These restrictions do not guarantee the trustworthiness or privacy practices of an external server.

If you choose to remember an SVN password, the app stores it using operating-system protection tied to the current Windows user. Otherwise, the app manages it in memory during the running session. This does not mean that all documents, attachments, backups, or recovery files are encrypted. Manage access to your device account, disk, and backup locations as well.

SVN logout in the app removes login information managed by the app. It does not erase authentication caches in separate tools such as TortoiseSVN or commit history on the server.

SVN commands run by the app disable their own authentication caching and receive the password through an input stream. Authentication storage in tools you run separately must be managed in those tools. App logout may be restricted while operations or locks are active. Once completed, it deletes the current app-managed login information and its authentication storage.

GitHub updates and remote SVN connections allowed by the app use HTTPS, and update installer signatures are checked. App diagnostic-file fields are limited to avoid recording arbitrary document content, passwords, or full addresses. These limits do not apply identically to every recovery record or underlying-service diagnostic.

The operator limits access to inquiry materials to what is needed for handling them and minimizes unnecessary copying or forwarding. Inquiry retention and deletion follow section 5. This policy does not claim unverified additional authentication settings or encryption of all information.

Removing materials from a project or uninstalling the app can leave backups, exports, external folders, and recovery copies behind. Some recovery materials are stored in user folders outside the app package. Review the relevant storage locations and copies when complete removal is needed.

Local app data in the GitHub edition may remain after uninstalling. Uninstalling the Store edition may delete package-specific data, but editing input separately preserved in the user profile, external projects, and backups are distinct. Actual uninstall and reinstall testing of this release's Store package is not yet complete, so preservation or deletion in every situation is not guaranteed.

To remove local information, close the app normally, review and back up anything needed, and delete the files and copies at their respective storage locations through Windows. You can ask the support email for help locating them. Ordinary file deletion does not guarantee that information cannot be recovered from every storage medium.

## 7. Your choices and rights requests

You choose project and attachment contents and locations, backups, use of external URLs, SVN server connections, diagnostic exports, and whether to send email. However, displayed external content can connect before a separate playback action, as described in section 4.

You may request access, correction, deletion, restriction or suspension of processing, and withdrawal of consent for consent-based processing of inquiry information held by the operator, where provided by applicable law. Email [skais5384@naver.com](mailto:skais5384@naver.com) with your request and the minimum information needed to identify the relevant inquiry. The operator verifies your identity or authority to act only as necessary and does not routinely require a copy of an identity document from everyone.

Requests are handled within the procedures and time limits required by applicable law. If a legal restriction prevents all or part of a request from being fulfilled, the operator explains the reason and available next steps. You may ask for reconsideration at the same address or contact the applicable privacy regulator.

The operator cannot access or delete device files it does not hold or records controlled by your chosen SVN, cloud, or external-content service. Manage those files at their storage location or contact the relevant service provider.

## 8. Use by children and young people

The app is a general document and creative tool and is not designed specifically for a particular child age group. It does not have its own account registration or date-of-birth collection feature. This does not remove obligations that may apply to children's information.

If a child's inquiry or use of an external service requires parental permission or another safeguard under applicable rules, those requirements must be followed. An authorized parent or guardian may use the contact in section 7 where needed. An app-store content age rating is different from the age requirements for consent to personal-information processing.

## 9. External services' policies

The following official notices explain the relevant services' processing and user controls. They do not replace Dreamrugi's own disclosures or any required notice or consent.

- [Microsoft Privacy Statement](https://www.microsoft.com/en-us/privacy/privacystatement)
- [Microsoft WebView2 privacy guidance](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/data-privacy)
- [GitHub General Privacy Statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement)
- [Google Privacy Policy](https://policies.google.com/privacy)
- [YouTube Terms of Service](https://www.youtube.com/t/terms)
- [NAVER Privacy Policy](https://policy.naver.com/policy/privacy.html)

Content servers, SVN servers, cloud services, and external programs you select separately have their own practices.

## 10. Publication and changes

You can read this policy in Korean and English inside the app without an Internet connection, and in the privacy policy documents in the GitHub repository. A permanent policy page on the existing blog will also be linked when available. Revisions will be announced at the public policy location with the revision date, effective date, and key changes. Any separate advance notice or consent required by applicable law or service policies will be handled through the applicable procedure.

This does not mean that the app provides push notifications or automatic email notices. Revision records and effective dates will be identified separately.

| Version | Effective date | Key changes |
| --- | --- | --- |
| 1.0.0 | October 1, 2026 | Initial policy covering local information, external services, inquiries, and the choice for automatic YouTube loading. |
