//! The reducer: `update(state, msg) -> effects`. Pure: everything that
//! touches disk, servers or the engine is an [`Effect`] the runtime carries
//! out, reporting back with more [`Msg`]s.

use std::path::PathBuf;

use increart_core::config::{Config, MODEL_SLOTS, RecipePreset, SdServerConfig, ServerConfig, ServerPreset};
use increart_core::engine::{Event, Stop};
use increart_core::launcher::device_name;
use increart_core::scheduler::Order;
use increart_core::{Id, IdeaLoc, ImageRef, Index, NewIdea, Tree, Verdict};

use increart_core::HistoryKey;
use increart_core::config::{Resolution, TileFormat};
use increart_core::history::{HistoryEntry, HistoryKind};
use increart_core::model::{ImageEdits, Recipe, SourceInfo, StoreFormat};
use increart_core::model::{SweepParam, SweepTag};
use increart_core::pe::PeRun;
use increart_core::prompts::{BatchAsk, EnhanceAsk, PromptPreset, PromptPresets, SUGGEST_COUNT, SuggestAsk};

use crate::state::*;
use crate::tile::{Tile, TileContent};

// A few messages carry a whole library index; they are rare, so the size
// doesn't matter.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    // ---- start-up: library
    SetLibraryPath(String),
    OpenLibrary,
    CreateLibrary,
    /// Opens `config.recent_libraries[i]`.
    OpenRecentLibrary(usize),
    /// Drops `config.recent_libraries[i]` from the list (the folder stays).
    ForgetLibrary(usize),
    /// Opens a library wizard on a folder (`None` closes it).
    LibraryWizard(Option<(LibraryWizard, String)>),
    LibraryOpened(Result<(PathBuf, Index), String>),
    /// The background check of the images' usage counts after opening:
    /// images done, of how many (done = total: finished).
    UsageCheck(usize, usize),
    // ---- switching libraries (the top bar's library menu)
    /// Switch to this library (asking first while runs are generating).
    SwitchLibrary(String),
    SwitchLibraryNow(String),
    /// The library screen, to pick another library; with a wizard open.
    OtherLibrary(Option<(LibraryWizard, String)>),
    OtherLibraryNow(Option<(LibraryWizard, String)>),
    /// Back to the open library from the library screen.
    CancelSwitch,
    // ---- start-up: servers
    /// Opens a preset (asking first when servers are already in use).
    OpenPreset(usize),
    /// Opens a preset without asking.
    OpenPresetNow(usize),
    /// Opens the preset in the editor.
    EditPreset(usize),
    /// Opens the editor on an empty preset.
    NewPreset,
    CancelEdit,
    AddStartServer,
    AddConnectServer,
    /// A KoboldCpp image server, started by the app.
    AddKoboldServer,
    EditServer(usize, ServerConfig),
    RemoveServer(usize),
    SetPresetName(String),
    /// Saves the editor's preset (in place, renaming it if need be) and
    /// closes the editor.
    SavePreset,
    /// Save, then open it.
    SaveAndOpen,
    DeletePreset(usize),
    ListDevices,
    Devices(Result<Vec<String>, String>),
    /// Opens (lists the devices) or closes the server setup wizard.
    ServerWizard(bool),
    /// Wizard and editor: start a server on this device, or not.
    DraftDevice(String, bool),
    /// Wizard: save the draft as a preset (named after its devices) and open it.
    FinishServerWizard,
    /// Wizard: the KoboldCpp page (true) or the sd-server one.
    WizardKobold(bool),
    /// The KoboldCpp page's GPUs (`vulkaninfo`).
    /// The GPUs listed, and the RAM available (GiB).
    WizardGpus(Result<Vec<increart_core::tiers::Gpu>, String>, Option<f64>),
    /// The wizard's GPU runtime changed: its GPUs listed again.
    WizardRuntime(increart_core::pe::GpuRuntime),
    /// The KoboldCpp page edited (binary, folder, GPUs, layout).
    EditKoboldWizard(crate::state::KoboldWizard),
    /// The KoboldCpp page: download its models, then save and open.
    FinishKoboldWizard,
    /// **Save anyway**: KoboldCpp not found (yet); downloaded and saved as
    /// a preset, not opened.
    SaveKoboldWizard,
    /// **Save, download later**: saved as a preset, nothing fetched; the
    /// files to get by hand are said in the messages.
    SaveKoboldWizardLater,
    /// `--version` of the wizard's KoboldCpp binary, unless asked already.
    CheckWizardBinary,
    WizardDownload(crate::state::WizardDownload),
    /// Every model there (or not).
    WizardDownloaded(Result<(), String>),
    CancelWizardDownload,
    /// Opens the editor's servers without saving them (asking first when
    /// servers are already in use).
    OpenDraft,
    /// Opens the draft's servers without asking.
    OpenDraftNow,
    /// (attempt, server, result)
    ServerOpened(u32, usize, Result<(), String>),
    /// Every server of the draft has answered or failed.
    ServersSettled(u32),
    /// The servers [`Effect::StopServers`] (or [`Effect::OpenServers`])
    /// stopped are gone (the names of
    /// those the app had started; connected ones are only let go).
    ServersStopped(Vec<String>),
    /// What the open servers can load (by server name), for the model
    /// presets Generate offers.
    ServerModels(Vec<(String, increart_core::sdserver::Loadable)>),
    /// A starting server's latest log line (attempt, server).
    ServerLog(u32, usize, String),
    /// Leave the dialog while servers are still opening; they join when up.
    ContinueInBackground,
    /// Stop opening (and the servers already started).
    CancelOpening,
    ContinueWithServers,
    /// Without sd-servers, using the **Continue without servers** dialog's
    /// chat server (without the dialog, the one in use stays).
    ContinueWithoutServers,
    /// **Continue without servers**: its dialog (the chat server to use
    /// meanwhile), then `ContinueWithoutServers`.
    AskContinueWithoutServers,
    /// The dialog's choice; None closes it.
    WithoutDialog(Option<Without>),
    /// Leaves a server dialog opened from the main window, keeping the
    /// servers in use.
    CloseServerDialog,
    /// Shows or hides the server settings window.
    ServerSettings(bool),
    /// Shows or hides the server status window.
    ServerStatus(bool),
    /// Settings: loads wait for free RAM, or don't.
    SetSkipRamCheck(bool),
    /// Settings: the image servers' traffic is logged, or isn't.
    SetLogTraffic(bool),
    /// **Test** on a server the app connects to: opens the Test window.
    TestServer(ServerConfig),
    /// A test's step (by test number and index) started or ended.
    ServerTestStep(u32, usize, increart_core::servertest::Step),
    ServerTestDone(u32),
    /// Stops the test, keeping its window.
    StopServerTest,
    /// Closes the Test window, stopping the test.
    CloseServerTest,
    /// **Validate servers…** (on the startup server screen): opens the
    /// window, listing the suite's cases; or closes it (stopping a run).
    ApiTest(bool),
    /// The suite said something, in the list or run of that number.
    ApiTestEvent(u32, increart_core::apitest::Event),
    /// The suite's process ended: None, or why it said nothing more (it
    /// isn't there, it ended early).
    ApiTestEnded(u32, Option<String>),
    /// Checks (or unchecks) these cases.
    ApiTestCheck(Vec<usize>, bool),
    /// Shows a case's log (None: the run's).
    ApiTestSelect(Option<usize>),
    /// Shows one server preset's cases, checking those (None: all presets).
    ApiTestPreset(Option<String>),
    ApiTestRun(crate::state::ApiRun),
    /// Stops the run: the case going on, then the servers it started.
    ApiTestStop,
    /// **Validate now**: the window on this preset, its basic cases run.
    ValidateBasic(String),
    /// The window's advanced view (true) or basic one.
    ApiTestAdvanced(bool),
    /// After validating: closes the window and opens the preset shown.
    ApiTestContinue,
    /// The stamps file, read.
    Validations(increart_core::validation::Validations),
    /// The Settings window.
    Settings(bool),
    SetNotifications(increart_core::notify::Notifications),
    SettingsTab(crate::state::SettingsTab),
    /// The library's store format and counts (after `Effect::StoreInfo`
    /// or `Effect::SetStoreFormat`).
    StoreInfo(Result<crate::state::StoreInfo, String>),
    /// The format new store images are written in (this library only).
    SetStoreFormat(StoreFormat),
    /// Convert every store image to the library's store format (asks
    /// first).
    ConvertStore,
    ConvertStoreConfirmed,
    CancelConvert,
    /// A store conversion's progress: images done, of how many.
    Converting(usize, usize),
    /// Export library: asks where, then writes the archive.
    ExportLibrary(increart_core::archive::ArchiveFormat),
    /// **Save for a bug report…**: asks where, then writes the zip.
    SaveBugReport,
    /// The bug report zip written: where, and its size.
    BugReportSaved(Result<(PathBuf, u64), String>),
    /// Unpack a library archive: asks which and into which folder, unpacks
    /// it there and opens the library.
    UnpackLibraryArchive,
    /// An archive's progress: bytes done, of how many.
    Archiving(u64, u64),
    CancelArchive,
    /// Export library ended: the archive and its size.
    LibraryExported(Result<(PathBuf, u64), String>),
    /// An archive unpacked: the library.
    ArchiveUnpacked(Result<PathBuf, String>),
    /// A store conversion ended: images converted, left as they were (gone,
    /// or too large for WebP), failed (with why), and whether it was
    /// cancelled.
    Converted {
        converted: usize,
        skipped: usize,
        failed: Vec<String>,
        cancelled: bool,
    },
    /// Settings → Test: plays a sound now, whatever the alerts' state.
    TestSound(increart_core::notify::Sound),
    /// A sound played (the player) or not; `test`: from Test.
    SoundPlayed(Result<String, String>, bool),
    /// No alert sounds for these seconds (`None`: until the app restarts).
    Snooze(Option<f64>),
    Unsnooze,
    /// The app's window gained or lost the focus.
    Focused(bool),
    /// Closes the "no server is connected" warning.
    CloseNoServerWarning,
    /// Back to the server dialog from the main window (**Servers…**).
    OpenServerDialog,
    // ---- navigation
    Tab(Tab),
    InboxTab(InboxTab),
    ProjectsTab(ProjectsTab),
    // ---- ideas
    Draft(IdeaDraft),
    /// Add the draft: to the inbox, straight to the library, or keep it and make a project.
    AddIdea(AddIdea),
    DropFiles(Vec<PathBuf>),
    InboxCursor(usize),
    /// Idea inbox Grid: the search, filters and order (the cursor starts over).
    InboxFilter(InboxFilter),
    /// Review the idea under the inbox cursor (keys / buttons).
    ReviewCurrentIdea(Verdict),
    ReviewIdea(Id, Verdict),
    /// Save to input library: asks for the name (one idea) and folder.
    SaveIdeas(Vec<Id>),
    EditSaveIdeaName(String),
    SaveIdeaFolder(PathBuf),
    /// A new folder in the chosen one to save into (`None`: none).
    SaveIdeaNewFolder(Option<String>),
    SaveIdeaFilter(String),
    ConfirmSaveIdea,
    /// Save, then a project from what was saved: named as the idea is (the
    /// first idea's title of several), where the last project from ideas went, in
    /// its own folder; the New project dialog when that name won't do.
    SaveIdeaNewProject,
    CancelSaveIdea,
    /// Asks for the new project's name and folder, then makes it from these ideas.
    NewProjectFromIdeas(Vec<Id>),
    EditFromIdeasName(String),
    FromIdeasParent(PathBuf),
    /// Step 2's new folder (`None`: none).
    FromIdeasNewFolder(Option<String>),
    FromIdeasFilter(String),
    /// Collapses or expands a folder of the project tree.
    ToggleProjectFolder(PathBuf),
    /// Collapses (true) or expands every folder of the project tree.
    CollapseProjectFolders(bool),
    /// `true`: in the dialog's folder (Create in folder).
    ConfirmFromIdeas,
    CancelFromIdeas,
    Restore(Id, IdeaLoc),
    SetStar(Id, bool),
    DeleteDiscarded(Id),
    /// A discarded project back where it was (Projects → Discarded).
    RestoreProject(Id),
    /// A discarded project for good (asks first in the GUI).
    DeleteDiscardedProject(Id),
    /// Every discarded project for good (asks first in the GUI).
    EmptyDiscardedProjects,
    // ---- input library
    LibFolder(Option<PathBuf>),
    /// The idea in the Input library's side panel (`None` closes it).
    LibInfo(Option<Id>),
    LibSelect {
        id: Id,
        additive: bool,
    },
    LibClearSelection,
    // ---- trees
    NewFolder(Tree, PathBuf),
    NewProject(PathBuf),
    StartRename(Tree, PathBuf),
    EditRename(String),
    CommitRename,
    CancelRename,
    MoveNode(Tree, PathBuf, PathBuf),
    DeleteNode(Tree, PathBuf),
    // ---- projects
    SelectProject(Id),
    SelectProjectFolder(Option<PathBuf>),
    /// Stops prompt enhance's asks for the Generate form, then sends the
    /// message (one leaving the project, once asked about it).
    LeaveGenerate(Box<Msg>),
    Generate(GenerateForm),
    SelectRecipe(usize),
    /// Adds images to the Generate form's list as references.
    AddRefs(Vec<ImageRef>),
    /// Opens (true) or closes Generate's From library… picker.
    RefPicker(bool),
    RefPickerSource(RefSource),
    RefPickerText(String),
    RefPickerRatings(Ratings),
    /// Chooses an image in the picker, or unchooses it.
    RefPickerToggle(ImageRef),
    /// Adds the chosen images as references and closes the picker.
    RefPickerAdd,
    /// Removes the list's `i`th image.
    RemoveImage(usize),
    /// Sets the list's `i`th image's roles (`<imageN>` reference, start).
    ImageRoles {
        index: usize,
        reference: bool,
        start: bool,
    },
    MoveImage {
        from: usize,
        to: usize,
    },
    /// Renumber the prompt's `<imageN>` tags as proposed (or leave it).
    RetagConfirm,
    RetagDismiss,
    QueueGenerate,
    /// Queue, the references' names checked (or let be).
    QueueChecked,
    /// Queue, past the model check (the preset no open server can run).
    QueueModelChecked,
    /// Queue the Sweep mode's runs (`true`: a big one was confirmed).
    QueueSweep(bool),
    /// Jobs: how the queue shares the servers (saved in the config).
    JobOrder(Order),
    /// The sweep's "one at a time" question: `Some(true)` switches to
    /// sweep by sweep and queues, `Some(false)` queues as is, `None` doesn't.
    SweepOrderAnswer(Option<bool>),
    /// Stop a run: Finish & stop or Stop now.
    CancelRun(Id, Stop),
    /// Stop every active run.
    StopAll(Stop),
    /// Several runs: a project's or a sweep's (Jobs).
    CancelRuns(Vec<Id>, Stop),
    // ---- review
    ReviewScope(Option<Id>),
    /// Review one sweep of a project (its contact sheet).
    ReviewSweep(Id, String),
    /// Review one run of a project.
    ReviewRun(Id, Id),
    /// Review's scope panel: every project's runs, or only the selected one's.
    ReviewAllRuns(bool),
    /// How Review's scope tree orders folders and projects (kept in config).
    ReviewOrder(increart_core::config::ReviewOrder),
    /// Review's scope tree hides what has nothing to review (**hide
    /// reviewed**, kept in config).
    ReviewHideReviewed(bool),
    /// Review's scope panel: the tree's search text.
    ReviewSearch(String),
    /// The project tree lists the selected project's runs (kept in config).
    ProjectRuns(bool),
    /// The order of projects in the tree and grid (kept in config).
    ProjectOrder(increart_core::config::TreeOrder),
    /// The order of the project tree's folders (kept in config).
    FolderOrder(increart_core::config::TreeOrder),
    /// Selecting a project narrows the grid to its folder, when that holds
    /// others (**narrow to siblings**).
    NarrowToSiblings(bool),
    /// Shows or hides a right-side details panel (for this session).
    ShowInfo(InfoPanel, bool),
    /// The contact sheet's name filter.
    SheetFilter(String),
    /// The contact sheet's cell size; `save`: into the config (at the end
    /// of a drag).
    SheetZoom {
        size: u32,
        save: bool,
    },
    /// An image grid's tile size; `save`: into the config (not while
    /// dragging).
    GridZoom {
        grid: ZoomGrid,
        size: u32,
        save: bool,
    },
    /// The interface zoom in percent; `save`: into the config (not while
    /// dragging).
    UiZoom {
        percent: u32,
        save: bool,
    },
    /// Ctrl + / Ctrl −: the interface zoom to the next step up or down.
    UiZoomStep(bool),
    /// The contact sheet's row headers (kept in config).
    SheetRows(increart_core::config::SheetRows),
    /// How many rows the contact sheet shows side by side.
    SheetBands(usize),
    SheetCell(usize, usize),
    /// Move the contact sheet's selection by (lines, columns) as shown: with
    /// rows in bands, a line down is `bands` rows on, and a column past a
    /// band's last seed is the next band's first.
    SheetMove(isize, isize),
    /// Rate the selected cell's image (whatever its verdict).
    SheetRate(Verdict),
    /// Rate a row's unreviewed images.
    SheetRow(usize, Verdict),
    /// This many more seeds for every style with a Good image.
    SheetMoreSeeds(u64),
    /// The styles (or values) of the sweep's images picked, as a new sweep
    /// in Generate.
    SheetNewSweep,
    /// A run's prompt and settings into its project's Generate form.
    RunInGenerate(Id),
    /// An output's run settings and its seed into Generate as one image,
    /// e.g. to try it again larger.
    RecipeInGenerate(ImageRef),
    /// Review every project under a folder (relative to `projects/`).
    ReviewScopeFolder(PathBuf),
    ReviewCursor(usize),
    ReviewNext(isize),
    ReviewCurrent(Verdict),
    /// Re-rate (or send back to Review: `None`) an output by its link.
    ReviewLink(PathBuf, Option<Verdict>),
    UndoReview,
    ReviewView(ReviewView),
    /// `Space`: grid ↔ single.
    ToggleReviewGrid,
    /// `C`: pin the current image as A and show the next as B (again: leave).
    Compare,
    /// A and B change sides on screen (which one steps stays).
    SwapCompare,
    /// Rate the pinned image (A).
    ReviewPinned(Verdict),
    /// `Shift+C` / Compare with…: the picker, for the image to compare this
    /// one (or Review's current / the Output library's selected) against.
    CompareWith(Option<ImageRef>),
    /// The Output library's Grid compares against this (None: leave).
    OutputCompare(Option<ImageRef>),
    // ---- output library
    OutputTab(OutputTab),
    OutputSource(OutputSource),
    DeleteOutput(PathBuf),
    /// A discarded image's link back in its project (Output library →
    /// Discarded).
    RestoreOutput(PathBuf),
    /// A discarded image's link for good (asks first in the GUI).
    DeleteDiscardedOutput(PathBuf),
    /// Every discarded image's link for good (asks first in the GUI).
    EmptyDiscardedOutputs,
    /// Deletes the bad images the Grid shows.
    DeleteShownRejected,
    // ---- confirmation
    Ask(Confirm),
    ConfirmYes,
    ConfirmNo,
    /// The question's other way out ([`Confirm::fix`]).
    ConfirmFix,
    /// Queue with this model preset instead (the model warning's fix).
    QueueWithPreset(usize),
    // ---- from the runtime
    Scanned(Index),
    OpDone(Result<OpOk, String>, Option<Index>),
    Engine(Event),
    /// The engine (re)started and queued these runs' pending seeds.
    RunsQueued(Vec<Id>),
    DismissMessage(usize),
    /// Clears the status line's messages.
    ClearMessages,
    /// Something went wrong that isn't about one operation.
    Problem(String),
    /// A note for the status line ("Copied …").
    Info(String),
    /// Files or folders changed outside the app (the library watch).
    LibraryChanged,
    /// Files in `inbox_drop/` have stopped changing: import them.
    DropReady,
    /// How the library is being watched (for the Import tab).
    WatchStatus(String),
    // ---- folder view
    FvGo(PathBuf),
    FvBack,
    FvForward,
    FvUp,
    FvRefresh,
    FvListed(PathBuf, Vec<FvEntry>),
    /// Folders whose tiles came on screen and need their previews.
    FvWantPreviews(Vec<String>),
    /// Exactly these selected (rubber-band selection).
    FvSelectSet(std::collections::BTreeSet<String>),
    /// Arrow keys: move the cursor `by` entries; `extend` (Shift) selects
    /// the range from the anchor.
    FvMoveCursor {
        by: i32,
        extend: bool,
    },
    /// Typed letters: jump to the first name starting with them.
    FvTypeAhead(String),
    /// New folder / project in the folder view's folder, renamed in place.
    FvNewFolder,
    FvNewProject,
    /// Drag and drop: move (or with Ctrl, copy) folders, projects or image
    /// links (paths relative to the library root) into the folder `dest`.
    DropInto {
        paths: Vec<PathBuf>,
        dest: PathBuf,
        copy: bool,
    },
    /// Previews for folders in a directory: (name, images).
    FvPreviews(PathBuf, Vec<(String, Vec<ImageRef>)>),
    /// Click (`Replace`), Ctrl-click (`Toggle`), Shift-click (`Range`).
    FvSelect(String, SelectHow),
    FvSelectAll,
    FvClearSelection,
    FvMode(FvMode),
    FvIconSize(f32),
    FvShowMeta(bool),
    FvPathEdit(Option<String>),
    /// Open an entry: a folder navigates, an image opens in Review / the
    /// Output library.
    FvOpen(String),
    FvCopy,
    FvCut,
    /// Paste the clipboard into the current folder (or into a subfolder).
    FvPaste(Option<String>),
    /// Delete the selection (asks first).
    FvDelete,
    /// F2 on the selection.
    FvRename,
    /// Delete these (library-relative) entries: the answer to FvDelete.
    FvDeleteConfirmed(Vec<PathBuf>),
    // ---- file pickers
    Pick(PickKind),
    Picked(PickKind, Vec<PathBuf>),
    RemoveDraftImage(usize),
    /// Files dropped on the Generate panel: imported as references.
    DropRefs(Vec<PathBuf>),
    SetSdServer(SdServerConfig),
    /// The shared KoboldCpp binary (`Config::koboldcpp`).
    SetKoboldBinary(String),
    /// `Config::timeouts` (each at least 1 s); the app hands them to the
    /// clients ([`increart_core::config::set_timeouts`]).
    SetTimeouts(increart_core::config::Timeouts),
    /// `--version` of the shared KoboldCpp binary, unless known.
    CheckKoboldVersion,
    /// (binary, `--version`'s answer)
    KoboldVersion(String, Result<String, String>),
    // ---- drafts
    /// The GUI's clock (seconds); drives autosave.
    Tick(f64),
    /// Save pending drafts now (quitting).
    FlushDrafts,
    DraftSaved(Result<(), String>),
    /// Reset the Generate form (and its draft).
    ClearGenerate,
    // ---- outputs as inputs
    UseImage(ImageRef, UseTarget),
    /// `R`: the image to the input library as a ref (an image-only idea).
    ToInputLibrary(ImageRef),
    /// `I`: a new idea in the Idea inbox from the image and its prompt.
    NewIdeaFromImage(ImageRef),
    /// The selected project's choice for images from outputs.
    /// Remove the img2img start image from the form.
    ClearInit,
    /// The top-left back / forward (`Alt+←/→` and the mouse's side
    /// buttons outside folder views).
    NavBack,
    NavForward,
    /// Projects tab: the tree's search text.
    ProjectSearch(String),
    /// Opens the name dialog for a plain copy of the project.
    DuplicateProject(PathBuf),
    EditDuplicateName(String),
    ConfirmDuplicate,
    CancelDuplicate,
    /// Add seeds to a run (continuing its seed numbers) and queue them.
    MoreSeeds(Id, u64),
    /// Select a project and open its Generate form.
    OpenInGenerate(Id),
    // ---- prompt presets
    /// Show / hide the Presets window.
    PresetsPanel(bool),
    /// What the window lists.
    PresetsShow(PresetsView),
    PresetsSearch(String),
    /// Use a preset: replace the prompt, or insert at this character of it.
    ApplyPreset {
        task: usize,
        preset: usize,
        insert_at: Option<usize>,
    },
    PresetFavourite(usize, usize),
    /// Tick / untick a preset for the sweep (source: Selected).
    SweepToggle(usize, usize),
    /// The same by key (a tick whose preset is gone).
    SweepToggleKey(increart_core::prompts::PresetKey),
    /// Ticks (`true`) or unticks these presets (task, preset) for the sweep
    /// (source: Selected).
    SweepTicks(Vec<(usize, usize)>, bool),
    /// Unticks every preset.
    SweepClearTicks,
    /// The style sweep's **Choose styles** window (its source becomes the
    /// presets it had, ticked).
    StylesOpen,
    StylesShow(StyleView),
    StylesSearch(String),
    StylesFilter(StyleFilter),
    StylesSort(increart_core::config::StyleSort),
    /// Close Choose styles: OK (`true`) keeps the ticks, Cancel puts back
    /// the sweep as it was.
    StylesClose(bool),
    // ---- prompt enhance
    /// Ask the PE server for values for these blanks.
    PeSuggest(Vec<BlankKey>),
    /// New values for a blank in place of those offered (and none offered
    /// before).
    PeReroll(BlankKey),
    /// **Suggest…** on a blank: the dialog, set as last time (else as the
    /// quick ask would be).
    PeSuggestOpen(BlankKey),
    /// The dialog changed (its settings remembered with the blank), or
    /// closed (`None`).
    PeSuggestDialog(Option<SuggestDialog>),
    /// Ask with the dialog's settings.
    PeSuggestAsk,
    /// **Suggest for blanks…**: the dialog, set as last time, else with
    /// these (the shared blanks short of suggestions) ticked.
    PeBatchSuggestOpen(Vec<String>),
    /// The dialog changed (remembered with the sweep), or closed (`None`).
    PeBatchSuggestDialog(Option<BatchSuggest>),
    /// Ask for every blank ticked with the dialog's settings, and close it.
    PeBatchSuggestAsk,
    /// Forget the values offered for a blank (asks no longer avoid them).
    PeForgetOffered(BlankKey),
    /// Clear the values offered for a blank (asks still avoid them).
    PeClearSuggestions(BlankKey),
    PeStatus(PeStatus),
    /// How many of the PE servers are ready.
    PeReady(usize),
    /// The PE server (by name) taking an ask (by number).
    PeOn(u64, String),
    /// How an ask (by number) is getting on.
    PeProgress(u64, PeStep),
    /// The server's answer to an ask (or what went wrong).
    PeAnswer(u64, Result<String, String>),
    /// Stop these asks (queued or being answered).
    PeStop(Vec<u64>),
    /// Open the rewrite / describe window and ask.
    PeRewrite(PeTarget),
    /// The window's instructions and answer, as edited.
    PeTextEdit {
        how: String,
        text: String,
    },
    /// Ask again (with the instructions).
    PeTextRetry,
    /// Close the window, using the answer or not.
    PeTextClose(PeUse),
    /// Improve these styles' prompts with the default settings (empty:
    /// those of the styles that will run not enhanced yet).
    PeStyleEnhance(Vec<increart_core::prompts::PresetKey>),
    /// **Enhance style prompts…**: the dialog, set as last time, else with
    /// the styles not enhanced yet ticked.
    PeStyleOpen,
    /// The dialog changed (remembered with the sweep), or closed (`None`).
    PeStyleDialog(Option<increart_core::prompts::EnhanceAsk>),
    /// Improve every style ticked with the dialog's settings, and close it.
    PeStyleAsk,
    /// The blanks prompt enhance added to styles' prompts: kept (true), or
    /// made plain words.
    PeAddedBlanks(bool),
    /// The chat server in use changed: start it if it's to start then.
    PeLaunch,
    /// Start the chat server in use now (or connect to it).
    PeStart,
    /// Stop the PE server the app started (asks waiting are dropped).
    PeShutdown,
    /// **Prompt enhance settings** on the server dialog draft's chat server
    /// (its place in the draft; None: a new one).
    PeEditorOpen(Option<usize>),
    PeEditorEdit(increart_core::pe::PeServer),
    PeEditorSave,
    PeEditorClose,
    /// List the GPUs a runtime sees, for the settings.
    PeListGpus(increart_core::pe::GpuRuntime),
    PeGpus(increart_core::pe::GpuRuntime, Result<Vec<(u32, String)>, String>),
    /// Prompt enhance's system prompt for a job (empty: the default).
    PeSystemPrompt {
        job: String,
        text: String,
    },
    /// Prompt enhance's sampling for a job; `None`: the default.
    PeSampling {
        job: String,
        sampling: Option<increart_core::pe::Sampling>,
    },
    /// The Generate form has selected `prompt_select`.
    PromptSelected,
    /// **Edit presets…**, on this task and preset.
    OpenPresetEditor(usize, Option<usize>),
    EditPresetEditor(PresetsEditor),
    RestorePresetDefaults,
    SavePresetEditor,
    ClosePresetEditor,
    /// **Save as preset…** with the Generate prompt.
    OpenSavePreset,
    EditSavePreset(SavePresetDialog),
    ConfirmSavePreset,
    CancelSavePreset,
    // ---- prompt history
    OpenHistory(HistoryKey),
    HistoryLoaded(HistoryKey, Vec<HistoryEntry>),
    HistorySelect(usize),
    HistorySearch(String),
    HistoryCheckpointsOnly(bool),
    HistoryEveryEntry(bool),
    /// Restore the selected entry (with the negative prompt / title too).
    HistoryRestore(Restore),
    /// Settings changes only.
    HistorySettingsOnly(bool),
    /// The entries that led to a run (`None`: every entry).
    HistoryRunFilter(Option<Id>),
    /// Projects → a run: its project's history, at its queued checkpoint.
    RunHistory(Id),
    CloseHistory,
    // ---- Edit idea
    OpenIdea(Id),
    EditIdea(IdeaEdit),
    SaveIdea,
    CloseIdea,
    // ---- input library
    LibTab(LibTab),
    LibFilter(LibFilter),
    // ---- Import tab
    LoadImports,
    Imports(Vec<(Id, SourceInfo)>),
    ImportDropNow,
    // ---- resolutions
    OpenResolutions,
    /// The Scale dialog for the Generate size or a resolutions-editor row.
    OpenScale(ScaleTarget),
    EditScale(ScaleDialog),
    ApplyScale,
    CloseScale,
    /// Home's Start a new project dialog: open (`true`) or close it, its
    /// name and references, Create.
    StartProject(bool),
    EditStartProject(StartProjectDialog),
    ConfirmStartProject,
    /// The New project / Duplicate dialog's name, Create, Cancel.
    EditNewProjectName(String),
    ConfirmNewProject,
    CancelNewProject,
    /// Shows an image large (`None` closes it).
    Preview(Option<ImageRef>),
    /// The large preview's next (1) or previous (-1) image in
    /// [`AppState::preview_list`], selecting it there too.
    PreviewStep(isize),
    /// The large preview, opened from the contact sheet: the image in the
    /// same seed column `d` styles down (or up), skipping cells without one;
    /// the selected cell follows.
    PreviewRow(isize),
    /// Rates the image the large preview shows. In Review's queue (which
    /// it then leaves) the preview goes on to the next image, closing after
    /// the last; on the contact sheet it steps on; elsewhere it stays.
    PreviewRate(Verdict),
    /// The job details window for this run (`None` closes it).
    JobInfo(Option<Id>),
    /// A run's generation log, read.
    JobLog(Id, Vec<String>),
    EditResolutions(Vec<Resolution>),
    SaveResolutions,
    CloseResolutions,
    // ---- model presets
    OpenRecipes,
    EditRecipes(RecipeEditor),
    SaveRecipes,
    CloseRecipes,
    // ---- review / output library
    NeedRecipe(Id),
    RecipeLoaded(Id, Option<Recipe>),
    /// Idea panels: the input images' metadata, read once each.
    NeedInputMeta(Vec<Id>),
    InputMetaLoaded(Vec<(Id, InputMeta)>),
    /// An output image where it is: Review when unrated, else selected in
    /// the Output library under its project.
    GoToOutput(ImageRef),
    Checkerboard(bool),
    /// Jump to the newest unreviewed image.
    ReviewNewest,
    /// Move on through the unreviewed images by itself, or not.
    ReviewFollow(bool),
    /// How Follow paces them (saved).
    FollowPace(increart_core::config::FollowPace),
    /// Follow paused from its button, or held while the pointer is over the
    /// image.
    FollowPause(bool),
    FollowHold(bool),
    OutputFilter(OutputFilter),
    /// The Output library's search box: project names (`true`) or the
    /// images' titles and prompts; what's typed goes along.
    OutputSearchProjects(bool),
    OutputProjectSearch(String),
    /// The sources panel has shown the source Show outputs revealed.
    OutputRevealed,
    /// A Grid preset tab: its filters replace the current ones.
    OutputPreset(OutputPreset),
    /// Output library → Grid on this source, warning about active filters
    /// or an empty source (Show outputs on projects and project folders).
    ShowOutputs(OutputSource),
    ResetOutputFilters,
    DismissOutputNotice,
    OutputSelect(Option<ImageRef>),
    AddToCollection(PathBuf, Vec<ImageRef>),
    /// A new collection (opened for rename) holding these images.
    NewCollection(PathBuf, Vec<ImageRef>),
    SetCover(Id, Option<ImageRef>),
    /// Opens the title dialog for an output image (with its title), or
    /// closes it.
    TitleDialog(Option<ImageRef>),
    EditTitle(String),
    /// Saves the dialog's title (an empty one removes it).
    CommitTitle,
    // ---- picking and tiling (DESIGN.md, Tiling)
    /// Ctrl+click toggles, Shift+click picks the range from the last picked
    /// (or the cursor); `Replace` clears the picks (a plain click).
    PickImage(ImageRef, SelectHow),
    /// The rubber band's picks, or Ctrl+A's.
    PickSet(Vec<ImageRef>),
    RatePicked(Verdict),
    /// The Tile dialog, from the picks.
    OpenTile,
    /// The Tile dialog, from the images in Generate (all, parked too).
    TileGenerateImages,
    /// The Tile dialog, from these ideas' images (Idea inbox, Input library).
    TileIdeas(Vec<Id>),
    /// The Idea inbox Grid's picks.
    InboxPick(Vec<Id>),
    /// Save (Good, Neutral) or Discard the Idea inbox's picks.
    ReviewPickedIdeas(Verdict),
    /// The Input library's selection (Shift+click's range, the rubber band).
    LibSelectSet(Vec<Id>),
    /// The Tile dialog, from these (the clipboard's); with one open, they
    /// go after its last tile.
    TileStart(Vec<crate::tile::TileContent>),
    // ---- Crop / mark…
    /// The editor on an image; from Generate, the image's index there.
    OpenMarkup(ImageRef, Option<usize>),
    /// What is drawn now (one undo step).
    MarkupEdit(ImageEdits),
    MarkupUndo,
    MarkupRedo,
    MarkTools(MarkTools),
    /// Save the copy; `true`: and use it in Generate.
    MarkupSave(bool),
    CloseMarkup,
    CloseTile,
    TileSelect(Option<usize>),
    /// A tile to this (column, row).
    TileMove(usize, u32, u32),
    TileSpan(usize, u32, u32),
    TileCols(u32),
    TileReflow,
    TileRemove(usize),
    TileCaption(usize, String),
    /// A text block in an empty cell (column, row).
    TileAddText(u32, u32),
    TileText(usize, String),
    TileTextSize(usize, increart_core::config::TextSize),
    /// The title and subtitle above the grid.
    TileTitle(String, String),
    TileCaptions(CaptionFill),
    /// Every image tile's caption, a template (`AppState::caption_text`).
    TileCaptionFormat(String),
    TileSettings(increart_core::config::TileSettings),
    /// Images (or pasted ones) from a cell on, else after the last tile.
    TileAdd(Vec<crate::tile::TileContent>, Option<(u32, u32)>),
    /// The `i`th tile's inputs just before it.
    TileAddInputs(usize),
    /// Every image's inputs: in columns at the left (`true`) or each
    /// before its image.
    TileInputs(bool),
    /// The reference picker, its choices for the Tile window (from a cell).
    TilePicker(Option<(u32, u32)>),
    /// Compose and copy, save or add to the library.
    TileOut(TileTarget),
    TilesDone(TileTarget, Result<crate::tile::TileImage, String>),
    OpenExport(Vec<ImageRef>, String),
    EditExport(ExportDialog),
    RunExport,
    CloseExport,
    Exported(Result<usize, String>),
    // ---- file manager
    /// Open a folder (or a file's folder) in the system file manager; the
    /// path is relative to the library or absolute.
    ShowInFileManager(PathBuf),
    /// Open the log viewer, on this log (a file name in the log folder).
    OpenLogs(Option<String>),
    /// The log viewer's choices changed (`None`: closed).
    Logs(Option<LogsView>),
    /// Open a file in the text editor (`config.editor`).
    OpenInEditor(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddIdea {
    Inbox,
    Keep,
    NewProject,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    SaveConfig(Config, PathBuf),
    /// Plays a sound with the configured player (on a thread); answers
    /// `SoundPlayed`.
    PlaySound {
        sound: increart_core::notify::Sound,
        volume: f32,
        player: String,
        test: bool,
    },
    OpenLibrary {
        path: PathBuf,
        create: bool,
    },
    ListDevices(String),
    /// `binary --version`, answered with `KoboldVersion`.
    KoboldVersion(String),
    /// The GPUs and their memory, for the wizard's KoboldCpp page.
    DetectGpus(increart_core::pe::GpuRuntime),
    /// Fetches the files into `dir` (skipping those there, linking those
    /// found in `model_dirs` or Hugging Face's cache), reporting
    /// `WizardDownload`, then `WizardDownloaded`.
    DownloadModels {
        dir: String,
        files: Vec<increart_core::tiers::Download>,
        model_dirs: Vec<String>,
    },
    CancelDownload,
    /// Open these sd-servers, stopping the engine and the servers in use
    /// that aren't among them (the ones that are, opened with the same
    /// settings, are kept as they are).
    OpenServers {
        servers: Vec<ServerConfig>,
        shared: SdServerConfig,
        attempt: u32,
    },
    /// Start the engine with the servers that came up, and queue every run
    /// with pending seeds.
    StartEngine {
        shared: SdServerConfig,
        order: Order,
        skip_ram_check: bool,
    },
    /// Spread the servers over the queued runs, or finish them one by one.
    SetJobOrder(Order),
    /// Loads stop (or start again) waiting for free RAM.
    SetSkipRamCheck(bool),
    /// The image servers' traffic log goes on or off.
    SetLogTraffic(bool),
    /// Test `server` (with `models` to load one of; not loading or making
    /// an image when `generate` says why), answered with `ServerTestStep`s
    /// and `ServerTestDone`.
    TestServer {
        run: u32,
        server: ServerConfig,
        models: Vec<(String, increart_core::sdserver::ModelKey)>,
        generate: Result<(), String>,
    },
    StopServerTest,
    /// List the suite's cases for the config saved at `config`, answered
    /// with an `ApiTestEvent` (its plan, or an error) and `ApiTestEnded`.
    ApiTestList {
        run: u32,
        config: PathBuf,
    },
    /// Run the suite on `cases` (empty: all), answered with
    /// `ApiTestEvent`s and `ApiTestEnded`.
    ApiTestRun {
        run: u32,
        config: PathBuf,
        cases: Vec<String>,
    },
    ApiTestStop,
    /// Read the stamps file, answered with `Validations`.
    LoadValidations(PathBuf),
    /// Note in the stamps file that opening these settings (a
    /// fingerprint) asked whether to validate them.
    ValidationAsked(PathBuf, String),
    /// Stop the engine and the servers the app started.
    StopServers,
    /// Stop the engine only, keeping the servers (switching libraries).
    StopEngine,
    SavePresets(PromptPresets, PathBuf),
    /// Ask the PE servers (starting them first if needed; ones not among
    /// them stopped), the jobs shared among them in order, one at a time
    /// on each; each reports [`Msg::PeOn`] and [`Msg::PeProgress`] and
    /// answers [`Msg::PeAnswer`].
    PeAsk {
        servers: Vec<increart_core::pe::PeServer>,
        jobs: Vec<PeJob>,
    },
    /// Drop these asks if still queued, and stop the one being answered.
    PeStop(Vec<u64>),
    /// Stop the PE server the app started; [`Msg::PeStatus`] Off once it's
    /// down.
    PeShutdown,
    PeListGpus(increart_core::pe::GpuRuntime),
    Lib(LibOp),
    QueueRun(Id),
    CancelRun(Id, Stop),
    Rescan,
    /// Show the system file picker (desktop portal, else zenity).
    Pick {
        kind: PickKind,
        start: Option<PathBuf>,
    },
    SaveDraft(DraftSave),
    LoadHistory(HistoryKey),
    /// Adds an entry (stamped now), or replaces the newest (a merged edit).
    RecordHistory {
        key: HistoryKey,
        entry: Box<HistoryEntry>,
        replace_last: bool,
    },
    LoadRecipe(Id),
    /// Reads a run's generation log (answered with [`Msg::JobLog`]).
    ReadJobLog(Id, std::path::PathBuf),
    LoadInputMeta(Vec<Id>),
    LoadImports,
    Export {
        images: Vec<ImageRef>,
        dest: PathBuf,
        keep_metadata: bool,
        by_id: bool,
    },
    ShowInFileManager {
        path: PathBuf,
        command: String,
    },
    OpenInEditor {
        path: PathBuf,
        command: String,
    },
    /// List a folder for the folder view (library-relative).
    ListDir(PathBuf),
    /// Watch this library for changes made outside the app (replacing any
    /// earlier watch); runs on its own thread.
    WatchLibrary(PathBuf),
    /// Checks every store image's usage counts against the runs, one image
    /// at a time, reporting [`Msg::UsageCheck`]; runs on its own thread
    /// (it reads every image: seconds on a cold disk).
    CheckUsage,
    /// The library's store format and counts; answers `StoreInfo`.
    StoreInfo,
    /// Sets the library's store format; answers `StoreInfo`.
    SetStoreFormat(StoreFormat),
    /// Converts every store image to this format, on its own thread
    /// (re-encoding takes a while), reporting `Converting`, then `Converted`.
    ConvertStore(StoreFormat),
    /// Stops a conversion after the batch it is on.
    CancelConvert,
    /// Writes a bug report zip ([`increart_core::bugreport`]) of the config
    /// at `config`, the logs and what's in `model_dirs` to `dest`, on its
    /// own thread; answers `BugReportSaved`.
    SaveBugReport {
        config: Option<PathBuf>,
        model_dirs: Vec<String>,
        /// The stamps file: it and each preset's newest report go in.
        validations: Option<PathBuf>,
        dest: PathBuf,
    },
    /// Writes the library at `root` to `dest`, on its own thread, reporting
    /// `Archiving`, then `LibraryExported`.
    ExportLibrary {
        root: PathBuf,
        dest: PathBuf,
        format: increart_core::archive::ArchiveFormat,
    },
    /// Unpacks `archive` into `into`, on its own thread, reporting
    /// `Archiving`, then `ArchiveUnpacked`.
    UnpackArchive {
        archive: PathBuf,
        into: PathBuf,
    },
    CancelArchive,
    /// Composes a tiled image, on its own thread; answers `TilesDone`
    /// (a file is written there too).
    ComposeTiles {
        root: PathBuf,
        layout: crate::tile::TileLayout,
        settings: increart_core::config::TileSettings,
        to: TileTarget,
    },
    /// A few images from inside each of `names` (folders in `dir`); runs
    /// on its own thread.
    FolderPreviews {
        root: PathBuf,
        dir: PathBuf,
        names: Vec<String>,
    },
}

/// Where a tiled image goes.
#[derive(Clone, Debug, PartialEq)]
pub enum TileTarget {
    Clipboard,
    File(PathBuf),
    /// A new idea at the top of the input library.
    Library,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectHow {
    Replace,
    Toggle,
    Range,
}

/// Autosaved forms (DESIGN.md, Drafts).
#[derive(Clone, Debug, PartialEq)]
pub enum DraftSave {
    Generate(Id, Box<increart_core::model::GenerateDraft>),
    Idea(increart_core::model::IdeaDraftFile),
    ClearIdea,
}

/// Library changes, carried out in order by the runtime.
#[derive(Clone, Debug, PartialEq)]
pub enum LibOp {
    NewIdea(NewIdea),
    /// Keep a new idea and make a project from it in `folder`.
    /// The idea, then a project named so in this folder.
    NewIdeaNewProject(NewIdea, PathBuf, String),
    ImportFiles(Vec<PathBuf>),
    /// Import images into the input store (for the New idea draft).
    ImportImages(Vec<PathBuf>),
    /// Import images into the input store as Generate references.
    ImportRefs(Vec<PathBuf>),
    ReviewIdea(Id, Verdict),
    /// An inbox idea to the input library: retitled (when given), into this
    /// folder (made if missing).
    SaveToLibrary(Id, Option<String>, PathBuf),
    MoveIdea(Id, IdeaLoc),
    SetStar(Id, bool),
    DeleteDiscarded(Id),
    RestoreProject(Id),
    DeleteDiscardedProject(Id),
    /// Keep the ideas (if still in the inbox) and make one project from them.
    /// The ideas into a new project named so in this folder.
    NewProjectFromIdeas(Vec<Id>, PathBuf, String),
    CreateFolder(Tree, PathBuf),
    CreateProject(PathBuf, String),
    Rename(Tree, PathBuf, String),
    MoveNode(Tree, PathBuf, PathBuf),
    DeleteNode(Tree, PathBuf),
    CreateRun {
        project: Id,
        form: GenerateForm,
        preset: RecipePreset,
        model_dirs: Vec<String>,
        queue: bool,
        /// Handed back with the run, for the history's queued checkpoint.
        queued: Box<Queued>,
    },
    /// A sweep: a run per form (each with its prompt and label), tagged.
    CreateSweep {
        project: Id,
        /// Each with its model preset.
        runs: Vec<(GenerateForm, SweepTag, RecipePreset)>,
        model_dirs: Vec<String>,
        queue: bool,
        queued: Box<Queued>,
    },
    /// Rates output links (each to its own verdict; `None`: unrated), all
    /// in one op, so one rescan however many.
    ReviewOutputs {
        links: Vec<(PathBuf, Option<Verdict>)>,
        undo: bool,
    },
    /// Output links to the trash (the images stay until Clean up).
    DeleteOutputs(Vec<PathBuf>),
    RestoreOutputs(Vec<PathBuf>),
    DeleteDiscardedOutputs(Vec<PathBuf>),
    /// A new project in `parent` named after `name`, or a copy of the
    /// project at `copy_of` (relative to `projects/`); then the Generate form
    /// `form` opens on it.
    PrepareProject {
        parent: PathBuf,
        name: String,
        copy_of: Option<PathBuf>,
        form: GenerateForm,
        /// The references picker to open on the form.
        then: StartRefs,
    },
    IdeaFromOutput {
        image: Id,
        with_prompt: bool,
        loc: IdeaLoc,
    },
    /// A cropped and marked copy of `image` in `input/edited/`.
    EditedCopy {
        image: ImageRef,
        edits: ImageEdits,
        title: String,
        to: EditedTo,
    },
    /// A tiled image as a new idea in the input library.
    IdeaFromTiles {
        image: crate::tile::TileImage,
        sources: Vec<ImageRef>,
    },
    /// A plain copy of a project next to it (setup and draft, no images),
    /// named.
    CopyProject(PathBuf, String),
    AddSeeds(Id, u64),
    /// Add a variant to an idea's text.
    AddVariant(Id, increart_core::model::PromptVariant),
    /// Save an idea's title, text, tags and images (Edit idea).
    SaveIdea {
        id: Id,
        title: String,
        text: String,
        tags: Vec<String>,
        images: Vec<Id>,
    },
    ImportInboxDrop,
    AddToCollection(PathBuf, Vec<ImageRef>),
    NewCollection(PathBuf, Vec<ImageRef>),
    SetCover(Id, Option<ImageRef>),
    /// project, output image, title ("" removes it)
    SetTitle(Id, Id, String),
    CopyNode(Tree, PathBuf, PathBuf),
}

/// What a run or sweep was queued from: its history checkpoint once the
/// library has made the runs.
#[derive(Clone, Debug, PartialEq)]
pub struct Queued {
    pub project: Id,
    pub form: Box<increart_core::model::GenerateDraft>,
}

/// How much of a history entry Restore puts back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restore {
    /// The whole form, where the entry has it (else the texts).
    All,
    /// The prompt (or idea text).
    Text,
    /// The prompt and negative (or idea text and title).
    TextAndExtra,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OpOk {
    Done,
    Idea(Id),
    Tiled(Id),
    /// An edited copy's image, and where it goes.
    Edited(Id, EditedTo),
    /// A new project from ideas: the Generate panel is seeded from them.
    ProjectFromIdeas {
        project: Id,
        prompt: String,
        refs: Vec<ImageRef>,
        idea: Option<Id>,
    },
    Created(Tree, PathBuf),
    /// A project copied by Duplicate (already named): select it.
    Duplicated(PathBuf),
    Run(Id, Box<Queued>),
    /// A sweep's runs (its name).
    Sweep(Vec<Id>, String, Box<Queued>),
    /// Images imported for the draft.
    Imported(Vec<Id>),
    /// Images imported as references for the Generate form.
    ImportedRefs(Vec<Id>),
    /// Seeds added to a run: queue it.
    SeedsAdded(Id, u64),
    /// A project made by `PrepareProject`: open `form` on it.
    ProjectReady {
        path: PathBuf,
        form: Box<GenerateForm>,
        then: StartRefs,
    },
    /// (link path now, verdict before) of each rated link; `error`: why
    /// the rest weren't.
    Reviewed {
        done: Vec<(PathBuf, Option<Verdict>)>,
        undo: bool,
        error: Option<String>,
    },
}

/// Opens a library from the library screen. Switching: the open one is
/// just returned to, else the engine stops first (keeping the servers).
fn open_library(state: &mut AppState, path: PathBuf, create: bool) -> Vec<Effect> {
    let mut fx = vec![];
    if let Some(sw) = &mut state.switching {
        if state.library.as_ref() == Some(&path) {
            return update(state, Msg::CancelSwitch);
        }
        if !sw.stopped && state.engine_running {
            sw.stopped = true;
            fx.push(Effect::StopEngine);
        }
    }
    fx.push(Effect::OpenLibrary { path, create });
    fx
}

/// Runs `then`, asking first while runs are generating (switching stops
/// them; they continue when their library is opened again).
fn ask_while_generating(state: &mut AppState, then: Msg) -> Vec<Effect> {
    let n = state.active_runs.len();
    if n == 0 {
        return update(state, then);
    }
    let text = format!(
        "{n} run(s) are generating in this library. Switching stops them: the images being \
         made now are dropped, and everything left is generated when you open this library again."
    );
    state.confirm = Some(Confirm::new(text, "Switch library", then));
    vec![]
}

/// The warning before queueing a model preset no open server can run
/// (e.g. sd-server's int8 files on KoboldCpp), with each server's reason.
fn model_warning(state: &AppState) -> Option<String> {
    let f = &state.generate;
    let recipes = &state.config.recipes;
    let used: Vec<&RecipePreset> = if f.sweep.on && f.sweep.vary == increart_core::prompts::SweepVary::Model {
        recipes.iter().filter(|r| f.sweep.models.contains(&r.name)).collect()
    } else {
        recipes.get(f.preset).into_iter().collect()
    };
    let lines: Vec<String> = used
        .iter()
        .filter_map(|r| state.why_no_server_runs(&r.files).map(|why| format!("{}:\n  {}", r.name, why.join("\n  "))))
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "No open server can run {}:\n\n{}\n\nQueued anyway, a server still tries to load it: that can fail, \
         or (KoboldCpp with an int8 file) pin tens of GB of RAM.",
        if lines.len() == 1 { "this model preset" } else { "these model presets" },
        lines.join("\n\n")
    ))
}

/// A fresh state for another library, keeping what isn't the library's:
/// settings, servers, presets, messages.
fn fresh_for_library(state: &mut AppState) {
    let mut fresh = AppState::new(state.config.clone(), state.config_path.clone());
    std::mem::swap(state, &mut fresh);
    let old = fresh;
    state.servers = old.servers;
    state.server_models = old.server_models;
    state.gpu_runtime = old.gpu_runtime;
    // the chat servers stay too (their asks were the old library's)
    let pe = old.pe;
    state.pe.server = pe.server;
    state.pe.more = pe.more;
    state.pe.status = pe.status;
    state.pe.ready = pe.ready;
    state.pe.gpus = pe.gpus;
    state.pe.gpus_runtime = pe.gpus_runtime;
    state.pe.starting_since = pe.starting_since;
    state.pe.last_load = pe.last_load;
    state.pe.read_times = pe.read_times;
    // (answers still coming carry numbers from before)
    state.pe.asks = pe.asks;
    state.engine_running = old.engine_running;
    state.open_attempt = old.open_attempt;
    state.background = old.background;
    state.no_server_warned = old.no_server_warned;
    state.checkerboard = old.checkerboard;
    state.review_all_runs = old.review_all_runs;
    state.prompt_presets = old.prompt_presets;
    state.presets_path = old.presets_path;
    state.busy = old.busy;
    state.saving = old.saving;
    state.now = old.now;
    state.messages = old.messages;
    // (inside `update`: its nesting goes on)
    state.nav.depth = old.nav.depth;
}

fn save_presets(state: &AppState) -> Vec<Effect> {
    state.presets_path.iter().map(|p| Effect::SavePresets(state.prompt_presets.clone(), p.clone())).collect()
}

/// Names given and unique (tasks; presets within a task), prompts given
/// and keeping to their style task's slot rule.
fn check_presets(list: &PromptPresets) -> Result<(), String> {
    let mut tasks = std::collections::HashSet::new();
    for t in &list.tasks {
        let name = t.name.trim();
        if name.is_empty() {
            return Err("A task has no name".into());
        }
        if !tasks.insert(name.to_lowercase()) {
            return Err(format!("Two tasks are called {name}"));
        }
        let mut names = std::collections::HashSet::new();
        for p in &t.presets {
            let n = p.name.trim();
            if n.is_empty() {
                return Err(format!("A preset in {name} has no name"));
            }
            if !names.insert(n.to_lowercase()) {
                return Err(format!("{name} has two presets called {n}"));
            }
            if p.prompt.trim().is_empty() {
                return Err(format!("{name} → {n} has no prompt"));
            }
            if let Some(why) = increart_core::prompts::slot_rule(name, &p.prompt) {
                return Err(format!("{name} → {n} {why}"));
            }
        }
    }
    Ok(())
}

/// An event of the suite, into its window.
fn api_test_event(t: &mut crate::state::ApiTest, event: increart_core::apitest::Event) {
    use increart_core::apitest::Event;
    let case = |t: &mut crate::state::ApiTest, id: &str| t.index(id);
    match event {
        // a run's plan is the cases it runs, of those listed: kept as listed
        Event::Plan { cases } if !t.running => {
            let was: std::collections::HashMap<String, bool> =
                t.cases.iter().map(|c| (c.case.id.clone(), c.checked)).collect();
            let first = t.cases.is_empty();
            t.cases = cases
                .into_iter()
                .map(|c| crate::state::ApiCase {
                    checked: was.get(&c.id).copied().unwrap_or(c.not_applicable.is_none()),
                    case: c,
                    outcome: None,
                    running: false,
                    summary: String::new(),
                    secs: 0.0,
                    log: vec![],
                    traffic: vec![],
                })
                .collect();
            t.selected = None;
            if first {
                let preset = t.preset.take();
                t.show_preset(preset);
            }
        }
        Event::Plan { .. } => {}
        Event::Start { id } => {
            if let Some(i) = case(t, &id) {
                let c = &mut t.cases[i];
                c.running = true;
                c.outcome = None;
                c.summary.clear();
                c.log.clear();
                c.traffic.clear();
                if t.follow {
                    t.selected = Some(i);
                }
            }
        }
        Event::Log { id: Some(id), text } => match case(t, &id) {
            Some(i) => t.cases[i].log.push(text),
            None => t.log.push(text),
        },
        Event::Log { id: None, text } => t.log.push(text),
        Event::Traffic { id: Some(id), text } => {
            if let Some(i) = case(t, &id) {
                t.cases[i].traffic.push(text);
            }
        }
        Event::Traffic { id: None, .. } => {}
        Event::End { id, outcome, summary, secs } => {
            if let Some(i) = case(t, &id) {
                let c = &mut t.cases[i];
                c.running = false;
                c.outcome = Some(outcome);
                c.summary = summary;
                c.secs = secs;
            }
        }
        Event::Done { counts, report, zip } => {
            t.log.push(counts.line());
            t.done = Some((counts, report, zip));
            if t.follow {
                t.selected = None;
            }
        }
        Event::Error { text } => t.error = Some(text),
    }
}

fn save_config(state: &AppState) -> Vec<Effect> {
    state.config_path.iter().map(|p| Effect::SaveConfig(state.config.clone(), p.clone())).collect()
}

fn server_dialog() -> ServerDialog {
    ServerDialog {
        editing: None,
        draft: ServerPreset { name: String::new(), servers: vec![] },
        devices: None,
        progress: vec![],
        opening: false,
        started: 0.0,
        wizard: false,
        kobold: None,
        kobold_version: None,
        from_main: false,
        settings: false,
        notice: None,
        without: None,
    }
}

/// The command line's `--servers`: opens the preset (going on to the main
/// window while it starts) or continues without servers. An unknown or
/// unusable preset leaves the servers screen up, saying why.
fn skip_servers(state: &mut AppState, choice: StartupServers) -> Vec<Effect> {
    let StartupServers::Preset(asked) = choice else { return update(state, Msg::ContinueWithoutServers) };
    let name = if asked.eq_ignore_ascii_case("last") { state.config.last_preset.clone() } else { Some(asked.clone()) };
    let found = name.and_then(|n| state.config.server_presets.iter().position(|p| p.name.eq_ignore_ascii_case(&n)));
    let problem = match found.map(|i| &state.config.server_presets[i]) {
        None if asked.eq_ignore_ascii_case("last") => Some("no preset opened yet".to_string()),
        None => Some("no preset by that name".into()),
        Some(p) if p.servers.is_empty() => Some("the preset has no servers".into()),
        Some(p) if p.servers.iter().any(|s| matches!(s, ServerConfig::Start { .. })) => state.sdserver_problem(),
        Some(_) => None,
    };
    if let (Some(e), Phase::Servers(d)) = (problem, &mut state.phase) {
        d.notice = Some(format!("--servers {asked}: {e}"));
        return vec![];
    }
    let mut fx = update(state, Msg::OpenPresetNow(found.unwrap_or_default()));
    fx.extend(update(state, Msg::ContinueInBackground));
    fx
}

/// The first time a preset is opened with settings not validated (the
/// suite's basic cases never passed for them), asks whether to validate
/// them first; asked once per settings. None: nothing to ask.
fn ask_to_validate(state: &mut AppState, i: usize, p: &ServerPreset) -> Option<Vec<Effect>> {
    use increart_core::validation::{Status, fingerprint};
    let path = state.validations_path.clone()?;
    // the suite starts the servers itself: not with any open
    let idle = matches!(&state.phase, Phase::Servers(d) if !d.opening)
        && state.servers.is_empty()
        && state.background.is_none()
        && state.api_test.is_none();
    if !state.suite_found || !idle || p.servers.is_empty() || state.validations.was_asked(&state.config, p) {
        return None;
    }
    let said = match state.validations.status(&state.config, p) {
        Status::Passed(_) => return None,
        Status::Never => {
            format!("{} hasn't been validated: the server validation suite hasn't checked its servers.", p.name)
        }
        Status::Changed(s) => format!("{} was validated on {}, but its settings have changed since.", p.name, s.day()),
        Status::Failed(s) => format!("{} failed its basic validation on {}.", p.name, s.day()),
    };
    let fp = fingerprint(&state.config, p);
    state.validations.asked.push(fp.clone());
    let text = format!(
        "{said}\n\nStrongly recommended: validate it now. Each server answers, lists its models and makes a \
         small image, then a short run goes through them all, as the app uses them. It takes a few minutes \
         (the first image the longest); a problem is said case by case, with a report to send.\n\n\
         Asked once for these settings: the preset's mark on this screen says whether it's validated."
    );
    let ask = Confirm::new(text, "Validate now", Msg::ValidateBasic(p.name.clone()))
        .with_fix("Open without", Msg::OpenPreset(i))
        .recommending("Validate these servers first?");
    let mut fx = vec![Effect::ValidationAsked(path, fp)];
    fx.extend(update(state, Msg::Ask(ask)));
    Some(fx)
}

/// Opening a preset stops the sd-servers in use it doesn't have, and
/// restarts the engine (dropping the images being made) unless its
/// sd-servers are those in use: ask first when either happens.
fn ask_before_replacing(state: &mut AppState, draft: &ServerPreset, then: Msg) -> Vec<Effect> {
    let sd = draft.sd_servers();
    if sd_unchanged(state, &sd) || (state.servers.is_empty() && !state.engine_running) {
        return update(state, then);
    }
    let stops = stopping_for(state, &sd);
    let runs = state.active_runs.len();
    if stops.is_empty() && runs == 0 {
        return update(state, then);
    }
    let what = if draft.name.trim().is_empty() { "these servers".into() } else { draft.name.clone() };
    let mut text = match stops.len() {
        0 => format!("Open {what}?"),
        n if n == state.servers.len() => format!("Stop the servers in use and open {what}?"),
        _ => format!("Stop {} and open {what}?", stops.join(", ")),
    };
    let kept: Vec<String> = state.servers.iter().map(server_label).filter(|n| !stops.contains(n)).collect();
    if !kept.is_empty() && !sd.is_empty() {
        text.push_str(&format!(" {} stay as they are.", kept.join(", ")));
    }
    if runs > 0 && !sd.is_empty() {
        text.push_str(&format!(
            "\n\n{runs} run(s) are generating: the images being made now are dropped and made again; \
             the rest of the queue goes on once the new servers are up."
        ));
    } else if runs > 0 {
        text.push_str(&format!("\n\n{runs} run(s) are generating: they wait until a server is connected again."));
    }
    let verb = if stops.is_empty() { "Open" } else { "Stop and open" };
    update(state, Msg::Ask(Confirm::new(text, verb, then)))
}

/// Whether these sd-servers are the ones in use, all up and generating.
fn sd_unchanged(state: &AppState, sd: &[ServerConfig]) -> bool {
    !sd.is_empty()
        && state.engine_running
        && state.background.is_none()
        && state.servers_opened_with.as_ref() == Some(&state.config.programs.sd_server)
        && state.servers.len() == sd.len()
        && state.servers.iter().zip(sd).all(|(s, c)| s.config.as_ref() == Some(c))
}

/// The servers in use that opening `sd` stops: those it doesn't have, and
/// started ones when the sd-server settings changed since.
fn stopping_for(state: &AppState, sd: &[ServerConfig]) -> Vec<String> {
    let same_settings = state.servers_opened_with.as_ref() == Some(&state.config.programs.sd_server);
    let kept = |c: &ServerConfig| sd.contains(c) && (same_settings || !c.is_started());
    state.servers.iter().filter(|s| !s.config.as_ref().is_some_and(kept)).map(server_label).collect()
}

/// A server as "stopping …" names it: its device, else its name.
fn server_label(s: &ServerStatus) -> String {
    if s.device.is_empty() { s.name.clone() } else { s.device.clone() }
}

/// The chat server in use from now: the one before stops unless it runs as
/// the same process, and one to start with its preset starts.
fn use_chat(state: &mut AppState, mut chats: Vec<increart_core::pe::PeServer>) -> Vec<Effect> {
    let now = state.pe.server.iter().chain(&state.pe.more);
    let same = now.clone().count() == chats.len() && now.zip(&chats).all(|(a, b)| a.same_process(b));
    let mut fx = if same { vec![] } else { update(state, Msg::PeShutdown) };
    state.pe.server = (!chats.is_empty()).then(|| chats.remove(0));
    state.pe.more = chats;
    fx.extend(update(state, Msg::PeLaunch));
    fx
}

/// The PE servers to ask: `first` (the one in use) and the others
/// sharing the asks.
fn pool(state: &AppState, first: increart_core::pe::PeServer) -> Vec<increart_core::pe::PeServer> {
    let more: Vec<_> = state.pe.more.iter().filter(|m| !m.same_process(&first)).cloned().collect();
    std::iter::once(first).chain(more).collect()
}

/// The editor's preset into `config.server_presets`: in place when editing
/// one (renaming it), else added. Closes the editor; false (with an error
/// message) when the name is missing or taken.
fn save_draft(state: &mut AppState) -> bool {
    let Phase::Servers(d) = &mut state.phase else { return false };
    let Some(editing) = d.editing else { return false };
    let name = d.draft.name.trim().to_string();
    let taken =
        state.config.server_presets.iter().enumerate().any(|(i, p)| p.name == name && editing != Editing::Preset(i));
    if name.is_empty() || taken {
        let e = if taken { format!("There is already a preset named {name}") } else { "Name the preset first".into() };
        state.messages_error(e);
        return false;
    }
    d.draft.name = name.clone();
    for s in &mut d.draft.servers {
        s.full_paths(&mut vec![]);
    }
    let preset = d.draft.clone();
    d.editing = None;
    match editing {
        Editing::Preset(i) if i < state.config.server_presets.len() => {
            let old = std::mem::replace(&mut state.config.server_presets[i], preset);
            if state.config.last_preset.as_ref() == Some(&old.name) {
                state.config.last_preset = Some(name);
            }
        }
        _ => state.config.server_presets.push(preset),
    }
    true
}

/// `--version` for the settings, unless asked already for this binary.
fn kobold_version_once(state: &AppState) -> Vec<Effect> {
    let binary = kobold_binary(state);
    match &state.phase {
        Phase::Servers(d) if d.kobold_version.as_ref().is_none_or(|(b, _)| *b != binary) => {
            vec![Effect::KoboldVersion(binary)]
        }
        _ => vec![],
    }
}

/// `--list-devices` for the editor and settings, unless listed already (the
/// settings' **List devices** asks again).
fn list_devices_once(state: &AppState) -> Vec<Effect> {
    match &state.phase {
        Phase::Servers(d) if d.devices.is_none() && !state.config.programs.sd_server.binary.is_empty() => {
            vec![Effect::ListDevices(state.config.programs.sd_server.binary.clone())]
        }
        _ => vec![],
    }
}

/// A `Start` server on `device`, on the next free port.
fn add_start_server(draft: &mut ServerPreset, device: &str) {
    let server = ServerConfig::Start {
        name: format!("gpu{}", draft.sd_servers().len()),
        host: "127.0.0.1".into(),
        port: next_port(draft),
        device: device.into(),
        args: vec![],
    };
    draft.servers.push(server);
}

/// The KoboldCpp binary new entries start: the one in use
/// ([`increart_core::config::Config::known_koboldcpp`]), else `koboldcpp`.
pub fn kobold_binary(state: &AppState) -> String {
    state.config.known_koboldcpp().unwrap_or_else(|| "koboldcpp".into())
}

/// The wizard's draft named (its devices, numbered past those taken),
/// to be saved as a new preset; the wizard closed.
fn name_wizard_draft(state: &mut AppState) {
    let Phase::Servers(d) = &mut state.phase else { return };
    let base = wizard_name(&d.draft);
    let taken = |n: &str| state.config.server_presets.iter().any(|p| p.name == n);
    d.draft.name =
        increart_core::config::first_free(|i| if i == 1 { base.clone() } else { format!("{base} ({i})") }, taken);
    d.editing = Some(Editing::New);
    d.wizard = false;
}

/// The KoboldCpp binary the wizard names: typed, else `koboldcpp`.
pub fn wizard_binary(k: &KoboldWizard) -> String {
    match k.binary.trim() {
        "" => "koboldcpp".into(),
        b => b.to_string(),
    }
}

/// The wizard's preset name: its devices ("ROCm0 + ROCm1"), or "Running
/// server(s)" for connections only.
fn wizard_name(draft: &ServerPreset) -> String {
    let devices: Vec<String> = draft
        .servers
        .iter()
        .filter_map(|s| match s {
            ServerConfig::Start { device, .. } => Some(device.clone()),
            ServerConfig::Kobold(k) if s.is_started() => Some(format!("KoboldCpp {}", k.device()).trim().to_string()),
            ServerConfig::Kobold(_) | ServerConfig::Connect { .. } | ServerConfig::Chat(_) => None,
        })
        .collect();
    match (devices.len(), draft.sd_servers().len()) {
        (0, 0) => "Prompt enhance".into(),
        (0, 1) => "Running server".into(),
        (0, _) => "Running servers".into(),
        _ => devices.join(" + "),
    }
}

/// The next free port after the draft's `Start` servers (from 1234).
fn next_port(draft: &ServerPreset) -> u16 {
    use increart_core::kobold::{KoboldRun, KoboldServer};
    draft
        .servers
        .iter()
        .filter_map(|s| match s {
            ServerConfig::Start { port, .. }
            | ServerConfig::Kobold(KoboldServer { run: KoboldRun::Start { port, .. }, .. }) => Some(*port + 1),
            _ => None,
        })
        .max()
        .unwrap_or(1234)
}

pub fn update(state: &mut AppState, msg: Msg) -> Vec<Effect> {
    state.nav.depth += 1;
    let fx = apply(state, msg);
    state.nav.depth -= 1;
    if state.nav.depth == 0 && state.review_follow {
        follow_step(state);
    }
    if state.nav.depth == 0 && !state.nav.restoring && state.phase == Phase::Main {
        let now = state.place();
        if state.nav.current.as_ref() != Some(&now) {
            if let Some(c) = state.nav.current.replace(now) {
                state.nav.back.push(c);
                if state.nav.back.len() > 100 {
                    state.nav.back.remove(0);
                }
            }
            state.nav.forward.clear();
        }
    }
    fx
}

/// Goes to a place from the history, through the messages that go there.
fn go_to(state: &mut AppState, p: Place) -> Vec<Effect> {
    let mut fx = update(state, Msg::Tab(p.tab));
    fx.extend(update(state, Msg::InboxTab(p.inbox_tab)));
    fx.extend(update(state, Msg::LibTab(p.lib_tab)));
    fx.extend(update(state, Msg::ProjectsTab(p.projects_tab)));
    fx.extend(update(state, Msg::OutputTab(p.output_tab)));
    if p.lib_folder != state.lib_folder {
        fx.extend(update(state, Msg::LibFolder(p.lib_folder)));
    }
    match p.project {
        Some(id) if state.project.as_ref() != Some(&id) && state.index.project(&id).is_some() => {
            fx.extend(update(state, Msg::SelectProject(id)));
        }
        None if state.project.is_some() || state.proj_folder != p.proj_folder => {
            fx.extend(update(state, Msg::SelectProjectFolder(p.proj_folder)));
        }
        _ => {}
    }
    match p.review_folder {
        Some(f) if state.review_folder.as_ref() != Some(&f) => fx.extend(update(state, Msg::ReviewScopeFolder(f))),
        None if state.review_folder.is_some() || state.review_scope != p.review_scope => {
            fx.extend(update(state, Msg::ReviewScope(p.review_scope)));
        }
        _ => {}
    }
    // after the scope: a sweep's contact sheet needs its sweep in view
    if p.review_view != state.review_view {
        fx.extend(update(state, Msg::ReviewView(p.review_view)));
    }
    if p.output_source != state.output_source {
        fx.extend(update(state, Msg::OutputSource(p.output_source)));
    }
    fx
}

fn apply(state: &mut AppState, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::NavBack | Msg::NavForward => {
            let nav = &mut state.nav;
            let (from, to) =
                if msg == Msg::NavBack { (&mut nav.back, &mut nav.forward) } else { (&mut nav.forward, &mut nav.back) };
            let Some(p) = from.pop() else { return vec![] };
            if let Some(c) = nav.current.take() {
                to.push(c);
            }
            state.nav.restoring = true;
            let fx = go_to(state, p);
            state.nav.restoring = false;
            state.nav.current = Some(state.place());
            fx
        }
        Msg::ProjectSearch(q) => {
            state.project_search = q;
            vec![]
        }
        // ------------------------------------------------ library dialog
        Msg::SetLibraryPath(p) => {
            if let Phase::Library(d) = &mut state.phase {
                d.path = p;
            }
            vec![]
        }
        Msg::OpenLibrary | Msg::CreateLibrary => {
            let create = msg == Msg::CreateLibrary;
            let Phase::Library(d) = &mut state.phase else { return vec![] };
            if d.path.trim().is_empty() {
                d.error = Some("Enter a folder".into());
                return vec![];
            }
            d.busy = true;
            d.error = None;
            let path = PathBuf::from(d.path.trim());
            open_library(state, path, create)
        }
        Msg::OpenRecentLibrary(i) => {
            let (Phase::Library(d), Some(path)) = (&mut state.phase, state.config.recent_libraries.get(i)) else {
                return vec![];
            };
            if d.busy {
                return vec![];
            }
            d.path = path.clone();
            d.busy = true;
            d.error = None;
            let path = PathBuf::from(path);
            open_library(state, path, false)
        }
        Msg::ForgetLibrary(i) => {
            if i >= state.config.recent_libraries.len() {
                return vec![];
            }
            state.config.forget_recent_library(i);
            save_config(state)
        }
        Msg::LibraryWizard(w) => {
            if let Phase::Library(d) = &mut state.phase
                && !d.busy
            {
                d.error = None;
                d.wizard = w.map(|(kind, path)| {
                    d.path = path;
                    kind
                });
            }
            vec![]
        }
        Msg::LibraryOpened(Ok((root, index))) => {
            let switched = state.switching.take().is_some();
            let mut fx = vec![];
            if switched {
                // the old library's asks: not wanted any more
                let asked: Vec<u64> = state.pe.jobs.iter().map(|j| j.id).collect();
                if !asked.is_empty() {
                    fx.push(Effect::PeStop(asked));
                }
                fresh_for_library(state);
            }
            state.config.add_recent_library(root.to_string_lossy().into_owned());
            state.library = Some(root.clone());
            if let Some(d) = &index.idea_draft {
                state.draft = IdeaDraft::from_file(d);
            }
            for m in index.missing_outputs() {
                state.messages_error(m);
            }
            state.index = index;
            state.rebuild_usage();
            state.inbox_tab = if state.unreviewed_ideas().is_empty() { InboxTab::NewIdea } else { InboxTab::Grid };
            fx.extend(save_config(state));
            fx.push(Effect::WatchLibrary(root));
            fx.push(Effect::CheckUsage);
            if switched {
                // the servers stay; the engine restarts on this library
                state.phase = Phase::Main;
                if state.engine_running {
                    fx.push(Effect::StartEngine {
                        shared: state.config.programs.sd_server.clone(),
                        order: state.config.job_order,
                        skip_ram_check: state.config.skip_ram_check,
                    });
                }
                return fx;
            }
            state.phase = Phase::Servers(server_dialog());
            if let Some(choice) = state.startup_servers.take() {
                fx.extend(skip_servers(state, choice));
            }
            fx
        }
        Msg::LibraryOpened(Err(e)) => {
            if let Phase::Library(d) = &mut state.phase {
                d.busy = false;
                d.error = Some(e);
            }
            vec![]
        }

        // ------------------------------------------------ switching libraries
        Msg::SwitchLibrary(path) => {
            if state.library.as_deref() == Some(std::path::Path::new(&path)) {
                return vec![];
            }
            ask_while_generating(state, Msg::SwitchLibraryNow(path))
        }
        Msg::SwitchLibraryNow(path) => {
            if state.phase != Phase::Main {
                return vec![];
            }
            let mut fx = update(state, Msg::FlushDrafts);
            state.switching = Some(LibrarySwitch::default());
            state.phase = Phase::Library(LibraryDialog { path: path.clone(), busy: true, ..Default::default() });
            fx.extend(open_library(state, PathBuf::from(path), false));
            fx
        }
        Msg::OtherLibrary(w) => ask_while_generating(state, Msg::OtherLibraryNow(w)),
        Msg::OtherLibraryNow(w) => {
            if state.phase != Phase::Main {
                return vec![];
            }
            let fx = update(state, Msg::FlushDrafts);
            state.switching = Some(LibrarySwitch::default());
            state.phase = Phase::Library(LibraryDialog::default());
            if w.is_some() {
                update(state, Msg::LibraryWizard(w));
            }
            fx
        }
        Msg::CancelSwitch => {
            let Some(sw) = state.switching.take() else { return vec![] };
            state.phase = Phase::Main;
            if sw.stopped && state.engine_running {
                return vec![Effect::StartEngine {
                    shared: state.config.programs.sd_server.clone(),
                    order: state.config.job_order,
                    skip_ram_check: state.config.skip_ram_check,
                }];
            }
            vec![]
        }

        // ------------------------------------------------ server dialog
        Msg::OpenPreset(i) => match state.config.server_presets.get(i) {
            Some(p) => {
                let p = p.clone();
                if let Some(fx) = ask_to_validate(state, i, &p) {
                    return fx;
                }
                ask_before_replacing(state, &p, Msg::OpenPresetNow(i))
            }
            None => vec![],
        },
        Msg::OpenPresetNow(i) => {
            let (Phase::Servers(d), Some(p)) = (&mut state.phase, state.config.server_presets.get(i)) else {
                return vec![];
            };
            if d.opening {
                return vec![];
            }
            d.editing = None;
            d.draft = p.clone();
            update(state, Msg::OpenDraftNow)
        }
        Msg::EditPreset(i) => {
            if let (Phase::Servers(d), Some(p)) = (&mut state.phase, state.config.server_presets.get(i))
                && !d.opening
            {
                d.editing = Some(Editing::Preset(i));
                d.draft = p.clone();
                d.progress.clear();
                return list_devices_once(state);
            }
            vec![]
        }
        Msg::NewPreset => {
            if let Phase::Servers(d) = &mut state.phase
                && !d.opening
            {
                d.editing = Some(Editing::New);
                d.draft = ServerPreset { name: String::new(), servers: vec![] };
                d.progress.clear();
                return list_devices_once(state);
            }
            vec![]
        }
        Msg::CancelEdit => {
            if let Phase::Servers(d) = &mut state.phase {
                d.editing = None;
            }
            vec![]
        }
        Msg::AddStartServer | Msg::AddConnectServer => {
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            let n = d.draft.sd_servers().len();
            if msg == Msg::AddStartServer {
                add_start_server(&mut d.draft, &format!("ROCm{n}"));
            } else {
                d.draft.servers.push(ServerConfig::Connect {
                    name: format!("server{n}"),
                    url: "http://127.0.0.1:1234".into(),
                    device: String::new(),
                });
            }
            d.progress.clear();
            vec![]
        }
        Msg::AddKoboldServer => {
            use increart_core::kobold::{KoboldRun, KoboldServer};
            let binary = kobold_binary(state);
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            let n = d.draft.sd_servers().len();
            let run = KoboldRun::Start {
                binary,
                config: String::new(),
                host: "127.0.0.1".into(),
                port: next_port(&d.draft).max(5001),
                device: Default::default(),
                args: vec![],
            };
            d.draft.servers.push(ServerConfig::Kobold(KoboldServer {
                name: format!("kcpp{n}"),
                run,
                chat: None,
                swap: false,
            }));
            d.progress.clear();
            vec![]
        }
        Msg::EditServer(i, s) => {
            if let Phase::Servers(d) = &mut state.phase
                && let Some(slot) = d.draft.servers.get_mut(i)
            {
                *slot = s;
                d.progress.clear();
            }
            vec![]
        }
        Msg::RemoveServer(i) => {
            if let Phase::Servers(d) = &mut state.phase
                && i < d.draft.servers.len()
            {
                d.draft.servers.remove(i);
                d.progress.clear();
            }
            vec![]
        }
        Msg::SetPresetName(n) => {
            if let Phase::Servers(d) = &mut state.phase {
                d.draft.name = n;
            }
            vec![]
        }
        Msg::SavePreset => {
            if !save_draft(state) {
                return vec![];
            }
            save_config(state)
        }
        Msg::SaveAndOpen => {
            if !save_draft(state) {
                return vec![];
            }
            let mut fx = save_config(state);
            fx.extend(update(state, Msg::OpenDraft));
            fx
        }
        Msg::DeletePreset(i) => {
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            if i >= state.config.server_presets.len() {
                return vec![];
            }
            let gone = state.config.server_presets.remove(i);
            if state.config.last_preset.as_ref() == Some(&gone.name) {
                state.config.last_preset = None;
            }
            d.editing = match d.editing {
                Some(Editing::Preset(e)) if e == i => None,
                Some(Editing::Preset(e)) if e > i => Some(Editing::Preset(e - 1)),
                e => e,
            };
            save_config(state)
        }
        Msg::ListDevices => vec![Effect::ListDevices(state.config.programs.sd_server.binary.clone())],
        Msg::Devices(r) => {
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            // the wizard starts with every GPU ticked
            if d.wizard
                && d.draft.servers.is_empty()
                && let Ok(list) = &r
            {
                for l in list.iter().map(|l| device_name(l)).filter(|n| !n.eq_ignore_ascii_case("cpu")) {
                    add_start_server(&mut d.draft, l);
                }
            }
            d.devices = Some(r);
            vec![]
        }
        Msg::ServerWizard(open) => {
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            if d.opening {
                return vec![];
            }
            let mut fx = match d.kobold.as_ref().and_then(|k| k.downloading.as_ref()) {
                Some(_) => vec![Effect::CancelDownload],
                None => vec![],
            };
            d.wizard = open;
            d.kobold = None;
            d.editing = None;
            d.progress.clear();
            d.draft = ServerPreset { name: String::new(), servers: vec![] };
            if !open {
                return fx;
            }
            d.devices = None;
            fx.extend(update(state, Msg::WizardKobold(true)));
            fx
        }
        Msg::WizardKobold(on) => {
            let binary = kobold_binary(state);
            let models_dir = state
                .config
                .programs
                .sd_server
                .model_dirs
                .first()
                .cloned()
                .unwrap_or_else(|| std::env::var("HOME").map(|h| format!("{h}/models")).unwrap_or_default());
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            if !d.wizard || d.kobold.as_ref().is_some_and(|k| k.downloading.is_some()) {
                return vec![];
            }
            match &mut d.kobold {
                // detect again
                Some(k) if on => {
                    k.gpus = None;
                    return vec![Effect::DetectGpus(k.runtime)];
                }
                None if !on => return vec![],
                _ => {}
            }
            d.draft.servers.clear();
            if !on {
                d.kobold = None;
                d.devices = None;
                return update(state, Msg::ListDevices);
            }
            d.kobold = Some(Box::new(KoboldWizard {
                binary,
                models_dir,
                gpus: None,
                choose: false,
                ram_gib: None,
                layout: Default::default(),
                pe_gpus: vec![],
                pe_by_hand: false,
                runtime: state.gpu_runtime,
                downloading: None,
                open: true,
                error: None,
            }));
            let mut fx = vec![Effect::DetectGpus(state.gpu_runtime)];
            fx.extend(update(state, Msg::CheckWizardBinary));
            fx
        }
        Msg::WizardRuntime(runtime) => {
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            let Some(k) = d.kobold.as_mut().filter(|k| k.runtime != runtime) else { return vec![] };
            k.runtime = runtime;
            // numbered differently: listed again
            k.gpus = None;
            k.pe_gpus.clear();
            k.pe_by_hand = false;
            vec![Effect::DetectGpus(runtime)]
        }
        Msg::WizardGpus(r, ram_gib) => {
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            // (a stale one: listed already)
            let Some(k) = d.kobold.as_mut().filter(|k| k.gpus.is_none()) else { return vec![] };
            // the discrete ones ticked
            let gpu =
                |gpu: increart_core::tiers::Gpu| WizardGpu { on: !gpu.integrated, gpu, preset: None, by_hand: false };
            k.gpus = Some(r.map(|l| l.into_iter().map(gpu).collect()));
            k.ram_gib = ram_gib;
            vec![]
        }
        Msg::EditKoboldWizard(new) => {
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            if let Some(k) = &mut d.kobold
                && k.downloading.is_none()
            {
                let mut new = KoboldWizard { downloading: None, error: None, ..new };
                if new.layout != k.layout {
                    // the other layout's own default
                    (new.pe_gpus, new.pe_by_hand) = (vec![], false);
                } else if new.pe_gpus != k.pe_gpus {
                    new.pe_by_hand = true;
                }
                **k = new;
            }
            vec![]
        }
        Msg::CheckWizardBinary => {
            let Phase::Servers(d) = &state.phase else { return vec![] };
            let Some(k) = &d.kobold else { return vec![] };
            let binary = wizard_binary(k);
            match &d.kobold_version {
                Some((b, _)) if *b == binary => vec![],
                _ => vec![Effect::KoboldVersion(binary)],
            }
        }
        Msg::FinishKoboldWizard | Msg::SaveKoboldWizard | Msg::SaveKoboldWizardLater => {
            let open = matches!(msg, Msg::FinishKoboldWizard);
            let later = matches!(msg, Msg::SaveKoboldWizardLater);
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            let Some(k) = &mut d.kobold else { return vec![] };
            if k.downloading.is_some() {
                return vec![];
            }
            k.open = open;
            k.models_dir = increart_core::config::full_path(&k.models_dir);
            k.binary = increart_core::config::full_binary(&k.binary);
            if k.models_dir.is_empty() {
                k.error = Some("Choose a folder for the models.".into());
                return vec![];
            }
            match k.plan() {
                Err(e) => {
                    k.error = Some(e);
                    vec![]
                }
                Ok(plan) if later => {
                    k.error = None;
                    let to = std::path::Path::new(&k.models_dir).join(increart_core::tiers::MODEL_SUBDIR);
                    let names: Vec<String> = plan
                        .downloads
                        .iter()
                        .map(|d| {
                            d.file.trim_start_matches(&format!("{}/", increart_core::tiers::MODEL_SUBDIR)).to_string()
                        })
                        .collect();
                    k.downloading = Some((plan, WizardDownload::default()));
                    let fx = update(state, Msg::WizardDownloaded(Ok(())));
                    let said = format!(
                        "Saved without its models: put {} in {} (the setup wizard's Files lists where to get them, \
                         or downloads them when run again).",
                        names.join(", "),
                        to.display()
                    );
                    // (shown on the servers screen, kept in the messages)
                    if let Phase::Servers(d) = &mut state.phase {
                        d.notice = Some(said.clone());
                    }
                    state.messages_info(said);
                    fx
                }
                Ok(plan) => {
                    k.error = None;
                    let files = plan.downloads.clone();
                    k.downloading = Some((plan, WizardDownload::default()));
                    let model_dirs = state.config.programs.sd_server.model_dirs.clone();
                    vec![Effect::DownloadModels { dir: k.models_dir.trim().to_string(), files, model_dirs }]
                }
            }
        }
        Msg::WizardDownload(p) => {
            if let Phase::Servers(d) = &mut state.phase
                && let Some((_, at)) = d.kobold.as_mut().and_then(|k| k.downloading.as_mut())
            {
                *at = p;
            }
            vec![]
        }
        Msg::CancelWizardDownload => vec![Effect::CancelDownload],
        Msg::WizardDownloaded(r) => {
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            let Some(k) = &mut d.kobold else { return vec![] };
            let Some((plan, _)) = k.downloading.take() else { return vec![] };
            if let Err(e) = r {
                k.error = Some(if e == "cancelled" { "Download cancelled; what's there is kept.".into() } else { e });
                return vec![];
            }
            let dir = k.models_dir.trim().to_string();
            d.draft.servers = plan.servers;
            // the recipe's files are under the model directory
            let c = &mut state.config;
            c.programs.koboldcpp = k.binary.trim().to_string();
            let open = k.open;
            if !c.programs.sd_server.model_dirs.contains(&dir) {
                c.programs.sd_server.model_dirs.push(dir);
            }
            match c.recipes.iter_mut().find(|r| r.name == plan.recipe.name) {
                Some(r) => *r = plan.recipe,
                None => c.recipes.push(plan.recipe),
            }
            if open {
                return update(state, Msg::FinishServerWizard);
            }
            // saved, not opened: the servers screen shows it in the list
            name_wizard_draft(state);
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            d.kobold = None;
            update(state, Msg::SavePreset)
        }
        Msg::DraftDevice(device, on) => {
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            let has = |s: &ServerConfig| matches!(s, ServerConfig::Start { device: x, .. } if *x == device);
            if on && !d.draft.servers.iter().any(has) {
                add_start_server(&mut d.draft, &device);
            } else if !on {
                d.draft.servers.retain(|s| !has(s));
            }
            vec![]
        }
        Msg::FinishServerWizard => {
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            if !d.wizard || d.draft.servers.is_empty() {
                return vec![];
            }
            name_wizard_draft(state);
            update(state, Msg::SaveAndOpen)
        }
        Msg::OpenDraft => {
            let Phase::Servers(d) = &state.phase else { return vec![] };
            let draft = d.draft.clone();
            ask_before_replacing(state, &draft, Msg::OpenDraftNow)
        }
        Msg::OpenDraftNow => {
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            if d.draft.servers.is_empty() || d.opening {
                return vec![];
            }
            d.editing = None;
            for s in &mut d.draft.servers {
                s.full_paths(&mut vec![]);
            }
            let draft = d.draft.clone();
            let mut fx = vec![];
            if !draft.name.is_empty() && state.config.preset(&draft.name).is_some() {
                state.config.last_preset = Some(draft.name.clone());
                fx.extend(save_config(state));
            }
            fx.extend(use_chat(state, draft.chat_pool()));
            let sd = draft.sd_servers();
            // the sd-servers in use already: nothing to open
            if sd_unchanged(state, &sd) {
                state.phase = Phase::Main;
                return fx;
            }
            // a chat server only: the sd-servers in use stop
            if sd.is_empty() {
                state.phase = Phase::Main;
                state.open_attempt += 1;
                state.background = None;
                fx.push(stop_servers(state));
                state.servers.clear();
                return fx;
            }
            let stopping = stopping_for(state, &sd);
            let Phase::Servers(d) = &mut state.phase else { return fx };
            d.opening = true;
            d.started = state.now;
            d.progress = vec![ServerProgress::Opening; sd.len()];
            state.start_log = vec![String::new(); sd.len()];
            state.open_attempt += 1;
            state.background = None;
            state.stopping = stopping;
            state.servers = sd
                .iter()
                .map(|s| ServerStatus {
                    config: Some(s.clone()),
                    name: s.name().into(),
                    device: s.device(),
                    state: "opening".into(),
                    model: None,
                    idle_since: None,
                    worked: false,
                })
                .collect();
            state.servers_opened_with = Some(state.config.programs.sd_server.clone());
            fx.push(Effect::OpenServers {
                servers: sd,
                shared: state.config.programs.sd_server.clone(),
                attempt: state.open_attempt,
            });
            fx
        }
        Msg::ServerOpened(attempt, _, _) | Msg::ServersSettled(attempt) | Msg::ServerLog(attempt, _, _)
            if attempt != state.open_attempt =>
        {
            vec![]
        }
        Msg::ServerModels(found) => {
            state.server_models.extend(found);
            // the model chosen is one they run, if they run any
            // (the same model in another preset first, keeping the form's settings)
            if state.runnable_alternative(state.generate.preset).is_some() {
                runnable_preset(state);
                mark_generate(state);
                return vec![];
            }
            let runnable = state.runnable_recipes();
            if !runnable.is_empty() && !runnable.contains(&state.generate.preset) {
                return update(state, Msg::SelectRecipe(runnable[0]));
            }
            vec![]
        }
        Msg::ServersStopped(names) => {
            state.stopping.clear();
            if !names.is_empty() {
                state.messages_info(format!("Stopped sd-server {}: its GPU memory is free", names.join(", ")));
            }
            vec![]
        }
        Msg::ServerLog(_, i, line) => {
            let opening = match &state.phase {
                Phase::Servers(d) => d.progress.get(i),
                _ => state.background.as_ref().and_then(|p| p.get(i)),
            };
            if opening == Some(&ServerProgress::Opening) {
                if state.background.is_some()
                    && let Some(s) = state.servers.get_mut(i)
                {
                    s.state = format!("starting… {line}");
                }
                if let Some(l) = state.start_log.get_mut(i) {
                    *l = line;
                }
            }
            vec![]
        }
        Msg::ServerOpened(_, i, r) => {
            let progress = match &mut state.phase {
                Phase::Servers(d) if d.opening => Some(&mut d.progress),
                _ => state.background.as_mut(),
            };
            if let Some(p) = progress.and_then(|p| p.get_mut(i)) {
                *p = match &r {
                    Ok(()) => ServerProgress::Up,
                    Err(e) => ServerProgress::Failed(e.clone()),
                };
            }
            if state.background.is_some()
                && let Some(s) = state.servers.get_mut(i)
            {
                s.state = match &r {
                    Ok(()) => "up, connecting".into(),
                    Err(_) => "failed".into(),
                };
            }
            if let (Some(_), Err(e)) = (&state.background, r) {
                state.messages_error(e);
            }
            vec![]
        }
        Msg::ServersSettled(_) => {
            if let Some(progress) = state.background.take() {
                return start_with_up(state, &progress);
            }
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            d.opening = false;
            if d.progress.iter().all(|p| *p == ServerProgress::Up) {
                state.phase = Phase::Main;
                state.engine_running = true;
                state.servers.iter_mut().for_each(|s| (s.idle_since, s.worked) = (None, false));
                return vec![Effect::StartEngine {
                    shared: state.config.programs.sd_server.clone(),
                    order: state.config.job_order,
                    skip_ram_check: state.config.skip_ram_check,
                }];
            }
            vec![]
        }
        Msg::ContinueInBackground => {
            let Phase::Servers(d) = &state.phase else { return vec![] };
            if !d.opening {
                return vec![];
            }
            state.background = Some(d.progress.clone());
            for (s, p) in state.servers.iter_mut().zip(&d.progress) {
                s.state = match p {
                    ServerProgress::Opening => "starting…".into(),
                    ServerProgress::Up => "up, connecting".into(),
                    ServerProgress::Failed(_) => "failed".into(),
                };
            }
            state.phase = Phase::Main;
            vec![]
        }
        Msg::CancelOpening => {
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            if !d.opening {
                return vec![];
            }
            d.opening = false;
            d.progress.clear();
            state.open_attempt += 1;
            let fx = vec![stop_servers(state)];
            state.servers.clear();
            fx
        }
        Msg::ContinueWithServers => {
            let Phase::Servers(d) = &state.phase else { return vec![] };
            if d.opening || !d.progress.contains(&ServerProgress::Up) {
                return vec![];
            }
            let up: Vec<bool> = d.progress.iter().map(|p| *p == ServerProgress::Up).collect();
            state.servers = state.servers.iter().zip(up).filter(|(_, u)| *u).map(|(s, _)| s.clone()).collect();
            state.phase = Phase::Main;
            state.engine_running = true;
            state.servers.iter_mut().for_each(|s| (s.idle_since, s.worked) = (None, false));
            vec![Effect::StartEngine {
                shared: state.config.programs.sd_server.clone(),
                order: state.config.job_order,
                skip_ram_check: state.config.skip_ram_check,
            }]
        }
        Msg::ContinueWithoutServers => {
            let chat = match &mut state.phase {
                Phase::Servers(d) if d.without.is_some() => d.without.take().and_then(|w| w.chat).into_iter().collect(),
                _ => state.pe.server.iter().chain(&state.pe.more).cloned().collect(),
            };
            state.phase = Phase::Main;
            let mut fx = vec![stop_servers(state)];
            state.servers.clear();
            fx.extend(use_chat(state, chat));
            fx
        }
        Msg::OpenServerDialog => {
            if state.phase == Phase::Main {
                state.server_status = false;
                state.phase = Phase::Servers(ServerDialog { from_main: true, ..server_dialog() });
            }
            vec![]
        }
        Msg::AskContinueWithoutServers => {
            // the chat server in use stays, unless another is picked
            let chat = state.pe.server.clone();
            update(state, Msg::WithoutDialog(Some(Without { chat })))
        }
        Msg::WithoutDialog(w) => {
            if let Phase::Servers(d) = &mut state.phase
                && !d.opening
            {
                d.without = w.map(Box::new);
            }
            vec![]
        }
        Msg::CloseServerDialog => {
            if matches!(&state.phase, Phase::Servers(d) if d.from_main && !d.opening) {
                state.phase = Phase::Main;
            }
            vec![]
        }
        Msg::ServerSettings(open) => {
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            d.settings = open && !d.opening;
            if !d.settings {
                // (a field closed while typing in it)
                return if state.config.full_paths().is_empty() { vec![] } else { save_config(state) };
            }
            let mut fx = list_devices_once(state);
            fx.extend(kobold_version_once(state));
            fx
        }
        Msg::ServerStatus(open) => {
            state.server_status = open && state.phase == Phase::Main;
            vec![]
        }
        Msg::SetSkipRamCheck(skip) => {
            if state.config.skip_ram_check == skip {
                return vec![];
            }
            state.config.skip_ram_check = skip;
            let mut fx = save_config(state);
            fx.push(Effect::SetSkipRamCheck(skip));
            fx
        }
        Msg::SetLogTraffic(on) => {
            if state.config.log_traffic == on {
                return vec![];
            }
            state.config.log_traffic = on;
            let mut fx = save_config(state);
            fx.push(Effect::SetLogTraffic(on));
            fx
        }
        Msg::TestServer(server) => {
            state.server_tests += 1;
            let run = state.server_tests;
            let running = state.server_test.as_ref().is_some_and(|t| !t.done);
            let url = server.base_url();
            let in_use: Vec<&str> = state
                .servers
                .iter()
                .filter(|s| s.config.as_ref().is_some_and(|c| c.base_url() == url))
                .map(|s| s.name.as_str())
                .collect();
            let generate = if state.engine_running && !in_use.is_empty() {
                Err(format!("not while the open servers use it ({}): close them first", in_use.join(", ")))
            } else {
                Ok(())
            };
            let models = state
                .config
                .recipes
                .iter()
                .map(|r| {
                    let key = increart_core::sdserver::ModelKey { recipe: r.recipe.clone(), models: r.files.clone() };
                    (r.name.clone(), key)
                })
                .collect();
            state.server_test =
                Some(crate::state::ServerTest { run, name: server.name().to_string(), steps: vec![], done: false });
            let mut fx = vec![];
            if running {
                fx.push(Effect::StopServerTest);
            }
            fx.push(Effect::TestServer { run, server, models, generate });
            fx
        }
        Msg::ServerTestStep(run, i, step) => {
            if let Some(t) = state.server_test.as_mut().filter(|t| t.run == run) {
                if i < t.steps.len() {
                    t.steps[i] = step;
                } else {
                    t.steps.push(step);
                }
            }
            vec![]
        }
        Msg::ServerTestDone(run) => {
            if let Some(t) = state.server_test.as_mut().filter(|t| t.run == run) {
                t.done = true;
            }
            vec![]
        }
        Msg::StopServerTest => match &state.server_test {
            Some(t) if !t.done => vec![Effect::StopServerTest],
            _ => vec![],
        },
        Msg::CloseServerTest => match state.server_test.take() {
            Some(t) if !t.done => vec![Effect::StopServerTest],
            _ => vec![],
        },
        Msg::ApiTest(true) => {
            // the suite starts the servers itself: not with any open
            let idle = matches!(&state.phase, Phase::Servers(d) if !d.opening)
                && state.servers.is_empty()
                && state.background.is_none();
            if !idle || state.api_test.is_some() {
                return vec![];
            }
            state.server_tests += 1;
            let run = state.server_tests;
            // on the last used preset, once its cases are listed
            let preset = state.config.last_preset.clone();
            let mut t = crate::state::ApiTest { run, follow: true, preset, ..Default::default() };
            let fx = match &state.config_path {
                Some(config) => {
                    t.listing = true;
                    vec![Effect::ApiTestList { run, config: config.clone() }]
                }
                None => {
                    t.error = Some("the config isn't saved anywhere: the suite reads it from its file".into());
                    vec![]
                }
            };
            state.api_test = Some(t);
            fx
        }
        Msg::ApiTest(false) => match &mut state.api_test {
            // stopped first; closed once it's over
            Some(t) if t.running => {
                let stop = !t.stopping;
                t.stopping = true;
                t.stopped = true;
                if stop { vec![Effect::ApiTestStop] } else { vec![] }
            }
            _ => {
                state.api_test = None;
                vec![]
            }
        },
        Msg::ApiTestEvent(run, event) => {
            if let Some(t) = state.api_test.as_mut().filter(|t| t.run == run) {
                api_test_event(t, event);
            }
            vec![]
        }
        Msg::ApiTestEnded(run, why) => {
            let mut fx = vec![];
            if let Some(t) = state.api_test.as_mut().filter(|t| t.run == run) {
                // a run may have noted a preset validated
                if t.running
                    && let Some(path) = &state.validations_path
                {
                    fx.push(Effect::LoadValidations(path.clone()));
                }
                if t.running {
                    t.ended = Some(state.now);
                }
                let listed = t.listing;
                t.listing = false;
                t.running = false;
                t.stopping = false;
                for c in t.cases.iter_mut().filter(|c| c.running) {
                    c.running = false;
                }
                if t.error.is_none() {
                    t.error = why;
                }
                // Validate now: its basic cases, once listed
                if listed && t.basic_pending && t.error.is_none() && !t.cases.is_empty() {
                    t.basic_pending = false;
                    t.check_basic();
                    fx.extend(update(state, Msg::ApiTestRun(crate::state::ApiRun::Checked)));
                }
            }
            fx
        }
        Msg::ValidateBasic(preset) => {
            if state.api_test.is_none() {
                let fx = update(state, Msg::ApiTest(true));
                if let Some(t) = &mut state.api_test {
                    t.preset = Some(preset);
                    t.basic_pending = true;
                }
                return fx;
            }
            let Some(t) = &mut state.api_test else { return vec![] };
            if t.running || t.listing || !t.presets().contains(&preset.as_str()) {
                return vec![];
            }
            t.show_preset(Some(preset));
            t.check_basic();
            update(state, Msg::ApiTestRun(crate::state::ApiRun::Checked))
        }
        Msg::Validations(v) => {
            state.validations = v;
            vec![]
        }
        Msg::ApiTestAdvanced(on) => {
            state.api_test_advanced = on;
            vec![]
        }
        Msg::ApiTestContinue => {
            let Some(t) = state.api_test.as_ref().filter(|t| !t.running && !t.listing) else { return vec![] };
            let Some(i) = t.preset.as_ref().and_then(|p| state.config.server_presets.iter().position(|x| &x.name == p))
            else {
                return vec![];
            };
            state.api_test = None;
            // validated (or chosen not to be): not asked again on opening
            let mut fx = vec![];
            let fp = increart_core::validation::fingerprint(&state.config, &state.config.server_presets[i]);
            if let Some(path) = &state.validations_path
                && !state.validations.asked.contains(&fp)
            {
                state.validations.asked.push(fp.clone());
                fx.push(Effect::ValidationAsked(path.clone(), fp));
            }
            fx.extend(update(state, Msg::OpenPresetNow(i)));
            fx
        }
        Msg::ApiTestCheck(ix, on) => {
            if let Some(t) = &mut state.api_test {
                for i in ix {
                    if let Some(c) = t.cases.get_mut(i) {
                        c.checked = on;
                    }
                }
            }
            vec![]
        }
        Msg::ApiTestPreset(preset) => {
            if let Some(t) = &mut state.api_test {
                t.show_preset(preset);
            }
            vec![]
        }
        Msg::ApiTestSelect(i) => {
            if let Some(t) = &mut state.api_test {
                t.selected = i.filter(|&i| i < t.cases.len());
                t.follow = false;
            }
            vec![]
        }
        Msg::ApiTestRun(which) => {
            let Some(config) = state.config_path.clone() else { return vec![] };
            let Some(t) = &mut state.api_test else { return vec![] };
            if t.running || t.listing || t.cases.is_empty() {
                return vec![];
            }
            let cases = t.ids(which);
            if cases.is_empty() && which != crate::state::ApiRun::All {
                return vec![];
            }
            state.server_tests += 1;
            t.run = state.server_tests;
            t.ran = t.picked(which);
            t.stopped = false;
            t.started = state.now;
            t.ended = None;
            t.running = true;
            t.follow = true;
            t.done = None;
            t.error = None;
            t.log.clear();
            vec![Effect::ApiTestRun { run: t.run, config, cases }]
        }
        Msg::ApiTestStop => match &mut state.api_test {
            Some(t) if t.running && !t.stopping => {
                t.stopping = true;
                t.stopped = true;
                vec![Effect::ApiTestStop]
            }
            _ => vec![],
        },
        Msg::Settings(open) => {
            state.settings_open = open && state.phase == Phase::Main;
            if state.settings_open && state.settings_tab == crate::state::SettingsTab::Library {
                return vec![Effect::StoreInfo];
            }
            vec![]
        }
        Msg::SettingsTab(tab) => {
            state.settings_tab = tab;
            if tab == crate::state::SettingsTab::Library { vec![Effect::StoreInfo] } else { vec![] }
        }
        Msg::StoreInfo(Ok(info)) => {
            state.store = Some(info);
            vec![]
        }
        Msg::StoreInfo(Err(e)) => {
            state.messages_error(format!("Image storage: {e}"));
            vec![]
        }
        Msg::SetStoreFormat(format) => {
            if state.converting.is_some() || state.store.is_some_and(|s| s.format == format) {
                return vec![];
            }
            vec![Effect::SetStoreFormat(format)]
        }
        Msg::ConvertStore => {
            let Some(info) = state.store.filter(|_| state.converting.is_none()) else { return vec![] };
            let (to, n) = match info.format {
                StoreFormat::Png => (StoreFormat::Png, info.counts.webp.0),
                StoreFormat::Webp => (StoreFormat::Webp, info.counts.png.0),
            };
            let mut text = format!(
                "Convert {n} image(s) to {}? Both formats are lossless: every image is checked to have exactly \
                 the same pixels before its old file goes, and its metadata and links move with it. It takes a \
                 while; the library stays usable meanwhile.",
                to.name()
            );
            if to == StoreFormat::Webp {
                text.push_str(" Versions of IncreArt before this one can't open a library holding WebP images.");
            }
            update(state, Msg::Ask(Confirm::new(text, "Convert", Msg::ConvertStoreConfirmed)))
        }
        Msg::ConvertStoreConfirmed => {
            let Some(info) = state.store.filter(|_| state.converting.is_none()) else { return vec![] };
            state.converting = Some((0, 0));
            vec![Effect::ConvertStore(info.format)]
        }
        Msg::CancelConvert => {
            if state.converting.is_some() {
                vec![Effect::CancelConvert]
            } else {
                vec![]
            }
        }
        Msg::ExportLibrary(format) => {
            if state.library.is_none() || state.archiving.is_some() {
                return vec![];
            }
            update(state, Msg::Pick(PickKind::LibraryArchiveOut(format)))
        }
        Msg::UnpackLibraryArchive => {
            if state.archiving.is_some() || matches!(&state.phase, Phase::Library(d) if d.busy) {
                return vec![];
            }
            update(state, Msg::Pick(PickKind::LibraryArchiveIn))
        }
        Msg::Archiving(done, total) => {
            if let Some(a) = &mut state.archiving {
                (a.done, a.total) = (done, total);
            }
            vec![]
        }
        Msg::CancelArchive => match state.archiving {
            Some(_) => vec![Effect::CancelArchive],
            None => vec![],
        },
        Msg::LibraryExported(r) => {
            state.archiving = None;
            match r {
                Ok((path, size)) => state.messages_info(format!(
                    "Exported the library to {} ({:.1} MB)",
                    path.display(),
                    size as f64 / 1e6
                )),
                Err(e) if e == ARCHIVE_STOPPED => state.messages_info("Export library stopped"),
                Err(e) => state.messages_error(format!("Export library: {e}")),
            }
            vec![]
        }
        Msg::ArchiveUnpacked(r) => {
            let archive = state.archiving.take().map(|a| a.archive).unwrap_or_default();
            match r {
                Ok(root) => {
                    state.messages_info(format!("Unpacked {} into {}", archive.display(), root.display()));
                    // the start-up library screen opens it as it would a folder
                    if let Phase::Library(d) = &mut state.phase {
                        d.path = root.to_string_lossy().into_owned();
                        return update(state, Msg::OpenLibrary);
                    }
                    update(state, Msg::SwitchLibrary(root.to_string_lossy().into_owned()))
                }
                Err(e) if e == ARCHIVE_STOPPED => {
                    state.messages_info("Unpacking stopped");
                    vec![]
                }
                Err(e) => {
                    let e = format!("Unpacking {}: {e}", archive.display());
                    match &mut state.phase {
                        Phase::Library(d) => d.error = Some(e),
                        _ => state.messages_error(e),
                    }
                    vec![]
                }
            }
        }
        Msg::Converting(done, total) => {
            state.converting = Some((done, total));
            vec![]
        }
        Msg::Converted { converted, skipped, failed, cancelled } => {
            state.converting = None;
            let mut text = format!("Converted {converted} image(s)");
            if skipped > 0 {
                text.push_str(&format!(", {skipped} left as they were (too large for WebP, or gone)"));
            }
            if cancelled {
                text.push_str("; stopped before the end");
            }
            if failed.is_empty() {
                state.messages_info(text);
            } else {
                let shown: Vec<_> = failed.iter().take(5).cloned().collect();
                state.messages_error(format!("{text}; {} failed: {}", failed.len(), shown.join("; ")));
            }
            match state.library {
                Some(_) => vec![Effect::StoreInfo, Effect::Rescan],
                None => vec![],
            }
        }
        Msg::SetNotifications(n) => {
            if state.config.notifications == n {
                return vec![];
            }
            state.config.notifications = n;
            save_config(state)
        }
        Msg::TestSound(sound) => {
            state.alerts.test_result = None;
            let n = &state.config.notifications;
            vec![Effect::PlaySound { sound, volume: n.volume, player: n.player.clone(), test: true }]
        }
        Msg::SoundPlayed(r, test) => {
            if test {
                state.alerts.test_result = Some(r.clone());
            }
            match r {
                Ok(_) => state.alerts.sound_error = None,
                Err(e) => {
                    if !test && state.alerts.sound_error.as_ref() != Some(&e) {
                        state.messages_error(format!("Notification sound: {e}"));
                    }
                    state.alerts.sound_error = Some(e);
                }
            }
            vec![]
        }
        Msg::Snooze(secs) => {
            match secs {
                Some(s) => state.alerts.snooze_until = Some(state.now + s),
                None => state.alerts.muted = true,
            }
            vec![]
        }
        Msg::Unsnooze => {
            state.alerts.snooze_until = None;
            state.alerts.muted = false;
            vec![]
        }
        Msg::Focused(f) => {
            state.alerts.focused = f;
            if f {
                state.alerts.cleared_text = None;
            }
            vec![]
        }
        Msg::CloseNoServerWarning => {
            state.no_server_warning = false;
            vec![]
        }

        // ------------------------------------------------ navigation
        Msg::Tab(t) => {
            if t != state.tab {
                state.picked.clear();
                state.inbox_picked.clear();
            }
            if t == Tab::IdeaInbox && state.tab != Tab::IdeaInbox {
                state.inbox_tab = if state.unreviewed_ideas().is_empty() { InboxTab::NewIdea } else { InboxTab::Grid };
            }
            state.tab = t;
            fv_sync(state);
            relist(state)
        }
        Msg::InboxTab(t) => {
            if t != state.inbox_tab {
                state.inbox_picked.clear();
            }
            state.inbox_tab = t;
            fv_sync(state);
            relist_if_stale(state)
        }
        Msg::ProjectsTab(t) => {
            state.projects_tab = t;
            fv_sync(state);
            if t == ProjectsTab::Folder && state.folder_view.listed.as_ref() != Some(&state.folder_view.path) {
                state.folder_view.listed = None;
                return relist(state);
            }
            vec![]
        }

        // ------------------------------------------------ ideas
        Msg::Draft(d) => {
            if d != state.draft {
                state.draft = d;
                state.idea_dirty = Some(state.now);
            }
            vec![]
        }
        Msg::AddIdea(how) => {
            if state.draft.is_empty() {
                state.messages_error("The idea is empty");
                return vec![];
            }
            if how == AddIdea::NewProject {
                // the draft stays until the project is named
                let name = from_ideas_name(state, [state.draft.title.as_str()]);
                state.from_ideas = Some(FromIdeasDialog::new(vec![], from_ideas_parent(state), name));
                return vec![];
            }
            if how == AddIdea::Keep {
                // the draft stays until it's named and has a folder
                let name = state.draft.title.clone();
                state.save_idea = Some(SaveIdeaDialog::new(state, vec![], name));
                return vec![];
            }
            add_idea(state, how, PathBuf::new(), String::new(), None)
        }
        Msg::DropFiles(paths) => {
            if paths.is_empty() || state.library.is_none() {
                return vec![];
            }
            state.busy += 1;
            vec![Effect::Lib(LibOp::ImportFiles(paths))]
        }
        Msg::InboxCursor(i) => {
            state.inbox_cursor = i.min(state.inbox_queue().len().saturating_sub(1));
            vec![]
        }
        Msg::InboxFilter(f) => {
            state.inbox_filter = f;
            state.inbox_cursor = 0;
            vec![]
        }
        Msg::ReviewCurrentIdea(v) => {
            let Some(e) = state.inbox_queue().get(state.inbox_cursor).map(|e| e.idea.id.clone()) else {
                return vec![];
            };
            update(state, Msg::ReviewIdea(e, v))
        }
        Msg::ReviewIdea(id, v) => lib(state, LibOp::ReviewIdea(id, v)),
        Msg::SaveIdeas(ids) => {
            if ids.is_empty() {
                return vec![];
            }
            let name = match &ids[..] {
                [one] => state.index.idea(one).map(|e| e.idea.title.clone()).unwrap_or_default(),
                _ => String::new(),
            };
            state.save_idea = Some(SaveIdeaDialog::new(state, ids, name));
            vec![]
        }
        Msg::EditSaveIdeaName(n) => {
            if let Some(d) = &mut state.save_idea {
                d.name = n;
            }
            vec![]
        }
        Msg::SaveIdeaFolder(f) => {
            if let Some(d) = &mut state.save_idea {
                d.folder = f;
            }
            vec![]
        }
        Msg::SaveIdeaNewFolder(f) => {
            if let Some(d) = &mut state.save_idea {
                d.new_folder = f;
            }
            vec![]
        }
        Msg::SaveIdeaFilter(f) => {
            if let Some(d) = &mut state.save_idea {
                d.filter = f;
            }
            vec![]
        }
        Msg::ConfirmSaveIdea => {
            let Some(d) = state.save_idea.take() else { return vec![] };
            if let Some(e) = save_idea_problem(&d) {
                state.messages_error(e);
                state.save_idea = Some(d);
                return vec![];
            }
            let (folder, _) = save_idea_folder(state, &d);
            state.save_idea_last = Some(folder.clone());
            if d.ideas.is_empty() {
                return add_idea(state, AddIdea::Keep, folder, d.name.trim().to_string(), None);
            }
            // the picks they were go with them
            state.inbox_picked.retain(|p| !d.ideas.contains(p));
            let title = (d.ideas.len() == 1).then(|| d.name.trim().to_string());
            d.ideas
                .into_iter()
                .flat_map(|id| lib(state, LibOp::SaveToLibrary(id, title.clone(), folder.clone())))
                .collect()
        }
        Msg::SaveIdeaNewProject => {
            let Some(d) = state.save_idea.clone() else { return vec![] };
            if let Some(e) = save_idea_problem(&d) {
                state.messages_error(e);
                return vec![];
            }
            let titles: Vec<String> = match &d.ideas[..] {
                [] | [_] => vec![d.name.trim().to_string()],
                ids => ids.iter().filter_map(|i| state.index.idea(i)).map(|e| e.idea.title.clone()).collect(),
            };
            let name = from_ideas_name(state, titles.iter().map(String::as_str));
            let mut p = FromIdeasDialog::new(d.ideas.clone(), from_ideas_parent(state), name);
            let mut fx = vec![];
            if d.ideas.is_empty() {
                // the draft: saved there as the project is made
                let (folder, _) = save_idea_folder(state, &d);
                state.save_idea = None;
                state.save_idea_last = Some(folder.clone());
                state.draft.title = d.name.trim().to_string();
                p.saved_in = Some(folder);
            } else {
                fx = update(state, Msg::ConfirmSaveIdea);
            }
            let ok = from_ideas_problem(state, &p).is_none();
            state.from_ideas = Some(p);
            if ok {
                fx.extend(update(state, Msg::ConfirmFromIdeas));
            }
            fx
        }
        Msg::CancelSaveIdea => {
            state.save_idea = None;
            vec![]
        }
        Msg::NewProjectFromIdeas(ids) => {
            if ids.is_empty() {
                return vec![];
            }
            let titles: Vec<String> =
                ids.iter().filter_map(|i| state.index.idea(i)).map(|e| e.idea.title.clone()).collect();
            let name = from_ideas_name(state, titles.iter().map(String::as_str));
            state.from_ideas = Some(FromIdeasDialog::new(ids, from_ideas_parent(state), name));
            vec![]
        }
        Msg::EditFromIdeasName(n) => {
            if let Some(d) = &mut state.from_ideas {
                if d.own_folder() {
                    d.new_folder = Some(n.trim().to_string());
                }
                d.name = n;
            }
            vec![]
        }
        Msg::FromIdeasParent(p) => {
            if let Some(d) = &mut state.from_ideas {
                d.parent = p;
            }
            vec![]
        }
        Msg::FromIdeasNewFolder(f) => {
            if let Some(d) = &mut state.from_ideas {
                d.new_folder = f;
            }
            vec![]
        }
        Msg::FromIdeasFilter(f) => {
            if let Some(d) = &mut state.from_ideas {
                d.filter = f;
            }
            vec![]
        }
        Msg::ConfirmFromIdeas => {
            let Some(d) = state.from_ideas.take() else { return vec![] };
            let name = d.name.trim().to_string();
            let (parent, _) = from_ideas_folder(state, &d);
            if let Some(e) = from_ideas_problem(state, &d) {
                state.messages_error(e);
                state.from_ideas = Some(d);
                return vec![];
            }
            // the next one goes next to a project's own folder, not in it
            state.from_ideas_last = Some(if d.own_folder() { d.parent.clone() } else { parent.clone() });
            if d.ideas.is_empty() {
                return add_idea(state, AddIdea::NewProject, parent, name, d.saved_in);
            }
            lib(state, LibOp::NewProjectFromIdeas(d.ideas, parent, name))
        }
        Msg::CancelFromIdeas => {
            state.from_ideas = None;
            vec![]
        }
        Msg::Restore(id, loc) => lib(state, LibOp::MoveIdea(id, loc)),
        Msg::SetStar(id, on) => lib(state, LibOp::SetStar(id, on)),
        Msg::DeleteDiscarded(id) => lib(state, LibOp::DeleteDiscarded(id)),
        Msg::RestoreProject(id) => lib(state, LibOp::RestoreProject(id)),
        Msg::DeleteDiscardedProject(id) => lib(state, LibOp::DeleteDiscardedProject(id)),
        Msg::EmptyDiscardedProjects => {
            let ids: Vec<Id> = state.index.discarded_projects.iter().map(|d| d.entry.project.id.clone()).collect();
            ids.into_iter().flat_map(|id| lib(state, LibOp::DeleteDiscardedProject(id))).collect()
        }

        // ------------------------------------------------ input library
        Msg::LibFolder(f) => {
            // on the Folder tab, the folder view goes there too
            let path = std::path::Path::new("input/library").join(f.clone().unwrap_or_default());
            state.lib_folder = f;
            state.lib_selected.clear();
            if state.tab == Tab::InputLibrary && state.lib_tab == LibTab::Folder {
                return fv_go(state, path, true);
            }
            vec![]
        }
        Msg::LibSelect { id, additive } => {
            if !additive {
                state.lib_selected.clear();
                state.lib_selected.insert(id);
            } else if !state.lib_selected.remove(&id) {
                state.lib_selected.insert(id);
            }
            vec![]
        }
        Msg::LibInfo(id) => {
            state.lib_info = id;
            vec![]
        }
        Msg::LibClearSelection => {
            state.lib_selected.clear();
            vec![]
        }

        // ------------------------------------------------ trees
        Msg::NewFolder(tree, parent) => lib(state, LibOp::CreateFolder(tree, parent)),
        Msg::NewProject(parent) => {
            // asks the name first; made on Create
            let mut name = "new project".to_string();
            for n in 2.. {
                if !state.project_name_taken(&parent.join("_"), &name) {
                    break;
                }
                name = format!("new project {n}");
            }
            state.renaming = Some(Renaming { tree: Tree::Projects, path: parent.join(&name), name, creating: true });
            vec![]
        }
        Msg::StartRename(tree, path) => {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            state.renaming = Some(Renaming { tree, path, name, creating: false });
            vec![]
        }
        Msg::EditRename(name) => {
            if let Some(r) = &mut state.renaming {
                r.name = name;
            }
            vec![]
        }
        Msg::CommitRename => match state.renaming.take() {
            Some(r) if r.creating => {
                let parent = r.path.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
                lib(state, LibOp::CreateProject(parent, r.name.trim().to_string()))
            }
            Some(r) if r.path.file_name().is_some_and(|n| n.to_string_lossy() != r.name) => {
                lib(state, LibOp::Rename(r.tree, r.path, r.name))
            }
            _ => vec![],
        },
        Msg::CancelRename => {
            state.renaming = None;
            vec![]
        }
        Msg::MoveNode(tree, path, parent) => lib(state, LibOp::MoveNode(tree, path, parent)),
        Msg::DeleteNode(tree, path) => lib(state, LibOp::DeleteNode(tree, path)),

        // ------------------------------------------------ projects
        Msg::SelectProject(id) => {
            if state.project.as_ref() != Some(&id) && ask_leaving_generate(state, &Msg::SelectProject(id.clone())) {
                return vec![];
            }
            // narrowed to its folder when that holds others (and asked);
            // else the grid keeps showing what it did, if the project is in it
            let shown = state.project_scope();
            let keep = state.index.project(&id).and_then(|p| {
                let narrow = !state.config.project_click_keeps_scope && state.has_siblings(p);
                (!narrow && p.dir.starts_with(std::path::Path::new("projects").join(&shown))).then_some(shown)
            });
            let mut fx = vec![];
            if state.project.as_ref() != Some(&id) {
                fx = leave_generate(state);
                state.project = Some(id);
                load_generate(state);
            }
            state.proj_folder = None;
            state.proj_scope_kept = keep;
            fx
        }
        Msg::SelectProjectFolder(Some(f)) if state.projects_tab == ProjectsTab::Folder => {
            if ask_leaving_generate(state, &Msg::SelectProjectFolder(Some(f.clone()))) {
                return vec![];
            }
            let mut fx = leave_generate(state);
            state.proj_folder = Some(f.clone());
            state.proj_scope_kept = None;
            state.project = None;
            fx.extend(fv_go(state, std::path::Path::new("projects").join(f), true));
            fx
        }
        Msg::SelectProjectFolder(f) => {
            if ask_leaving_generate(state, &Msg::SelectProjectFolder(f.clone())) {
                return vec![];
            }
            let fx = leave_generate(state);
            state.proj_folder = f;
            state.proj_scope_kept = None;
            state.project = None;
            fx
        }
        Msg::LeaveGenerate(then) => {
            let ids = generate_asks(state);
            let mut fx = if ids.is_empty() { vec![] } else { update(state, Msg::PeStop(ids)) };
            fx.extend(update(state, *then));
            fx
        }
        Msg::Generate(mut f) => {
            // changed only by prompt enhance's answers (a form drawn before
            // one came would undo it)
            f.sweep.suggestions = state.generate.sweep.suggestions.clone();
            f.sweep.batch_ask = state.generate.sweep.batch_ask.clone();
            f.sweep.enhance_ask = state.generate.sweep.enhance_ask.clone();
            if f != state.generate {
                state.generate = f;
                mark_generate(state);
            }
            vec![]
        }
        Msg::SelectRecipe(i) => {
            if let Some(p) = state.config.recipes.get(i).cloned() {
                state.generate.apply_preset(i, &p);
                mark_generate(state);
            }
            vec![]
        }
        Msg::RefPicker(true) => {
            let source = state.ref_picker_source.clone().unwrap_or_else(|| {
                let inputs =
                    state.index.ideas.iter().any(|e| matches!(e.loc, IdeaLoc::Library(_)) && !e.idea.images.is_empty());
                if inputs { RefSource::Inputs(None) } else { RefSource::Outputs(OutputSource::AllProjects) }
            });
            // the Tile window's inputs mean nothing to Generate
            let source = if source == RefSource::TileInputs { RefSource::Inputs(None) } else { source };
            state.ref_picker = Some(RefPicker {
                source,
                text: String::new(),
                ratings: Ratings::KEPT,
                chosen: vec![],
                target: PickTarget::Generate,
            });
            vec![]
        }
        Msg::TilePicker(at) => {
            let Some(t) = &state.tile else { return vec![] };
            let inputs = t.layout.images().any(|i| !state.image_inputs(i).is_empty());
            let source = match &state.ref_picker_source {
                _ if inputs => RefSource::TileInputs,
                Some(s) if *s != RefSource::TileInputs => s.clone(),
                _ => RefSource::Outputs(OutputSource::AllProjects),
            };
            state.ref_picker = Some(RefPicker {
                source,
                text: String::new(),
                ratings: Ratings::KEPT,
                chosen: vec![],
                target: PickTarget::Tile(at),
            });
            vec![]
        }
        Msg::RefPicker(false) => {
            state.ref_picker = None;
            vec![]
        }
        Msg::RefPickerSource(s) => {
            if let Some(p) = &mut state.ref_picker {
                p.source = s.clone();
                if !s.contextual() || s == RefSource::TileInputs {
                    state.ref_picker_source = Some(s);
                }
            }
            vec![]
        }
        Msg::RefPickerText(t) => {
            if let Some(p) = &mut state.ref_picker {
                p.text = t;
            }
            vec![]
        }
        Msg::RefPickerRatings(r) => {
            if let Some(p) = &mut state.ref_picker {
                p.ratings = r;
            }
            vec![]
        }
        // Compare with…: one click picks
        Msg::RefPickerToggle(img) if state.ref_picker.as_ref().is_some_and(|p| p.target == PickTarget::Compare) => {
            state.ref_picker = None;
            compare_against(state, img)
        }
        Msg::RefPickerToggle(img) => {
            let refs = state.generate.refs();
            if let Some(p) = &mut state.ref_picker
                && (p.target != PickTarget::Generate || !refs.contains(&img))
            {
                match p.chosen.iter().position(|c| *c == img) {
                    Some(i) => {
                        p.chosen.remove(i);
                    }
                    None => p.chosen.push(img),
                }
            }
            vec![]
        }
        Msg::RefPickerAdd => {
            let Some(p) = state.ref_picker.take() else { return vec![] };
            match p.target {
                PickTarget::Generate => update(state, Msg::AddRefs(p.chosen)),
                PickTarget::Tile(at) => {
                    let images = p.chosen.into_iter().map(TileContent::Image).collect();
                    update(state, Msg::TileAdd(images, at))
                }
                PickTarget::Compare => match p.chosen.into_iter().next() {
                    Some(a) => compare_against(state, a),
                    None => vec![],
                },
            }
        }
        Msg::CompareWith(image) => {
            match (state.tab, image) {
                (Tab::OutputLibrary, Some(img)) => state.output_selected = Some(img),
                (Tab::Review, Some(img)) => {
                    if let Some(i) = state.review_queue().iter().position(|o| o.image == img) {
                        state.review_cursor = i;
                        unfollow(state);
                    }
                }
                _ => {}
            }
            let Some(b) = state.compare_b() else { return vec![] };
            // what B was made from first, then its project's inputs
            let project =
                state.output_link(&b).and_then(|o| state.index.run(&o.name.run)).map(|(p, _)| p.project.id.clone());
            let source = if !state.image_inputs(&b).is_empty() {
                RefSource::ImageInputs(b)
            } else if let Some(id) = project.filter(|id| !state.project_inputs(id).is_empty()) {
                RefSource::ProjectInputs(id)
            } else {
                match &state.ref_picker_source {
                    Some(s) if !s.contextual() => s.clone(),
                    _ => RefSource::Outputs(OutputSource::AllProjects),
                }
            };
            state.ref_picker = Some(RefPicker {
                source,
                text: String::new(),
                ratings: Ratings::KEPT,
                chosen: vec![],
                target: PickTarget::Compare,
            });
            vec![]
        }
        Msg::OutputCompare(a) => {
            state.output_compare = a;
            vec![]
        }
        Msg::AddRefs(refs) => {
            let before = state.generate.refs();
            for r in refs {
                state.generate.add_reference(r);
            }
            images_changed(state, before)
        }
        Msg::RemoveImage(i) => {
            let before = state.generate.refs();
            if i < state.generate.images.len() {
                state.generate.images.remove(i);
            }
            images_changed(state, before)
        }
        Msg::ImageRoles { index, reference, start } => {
            let before = state.generate.refs();
            let images = &mut state.generate.images;
            if index < images.len() {
                if start {
                    // at most one start image
                    for (j, x) in images.iter_mut().enumerate() {
                        x.start = j == index;
                    }
                } else {
                    images[index].start = false;
                }
                images[index].reference = reference;
            }
            images_changed(state, before)
        }
        Msg::MoveImage { from, to } => {
            let before = state.generate.refs();
            let images = &mut state.generate.images;
            if from < images.len() && to < images.len() {
                let x = images.remove(from);
                images.insert(to, x);
            }
            images_changed(state, before)
        }
        Msg::RetagConfirm => {
            let Some(r) = state.retag.take() else { return vec![] };
            let mut fx = vec![];
            if let Some(p) = state.project.clone() {
                let (t, x) = (state.generate.prompt.clone(), state.generate.negative.clone());
                fx = record(state, HistoryKey::Project(p), HistoryKind::BeforeRetag, t, x);
            }
            state.generate.prompt = r.prompt;
            mark_generate(state);
            fx
        }
        Msg::RetagDismiss => {
            state.retag = None;
            vec![]
        }
        Msg::ClearGenerate => {
            let mut fx = vec![];
            if let Some(p) = state.project.clone() {
                let (t, x) = (state.generate.prompt.clone(), state.generate.negative.clone());
                fx = record(state, HistoryKey::Project(p), HistoryKind::BeforeClear, t, x);
            }
            let mut f = GenerateForm::new(&state.config);
            f.label = next_label(state);
            f.first_seed = state.next_seed();
            // the values offered for the sweep's blanks stay
            f.sweep.suggestions = std::mem::take(&mut state.generate.sweep.suggestions);
            f.sweep.batch_ask = state.generate.sweep.batch_ask.take();
            f.sweep.enhance_ask = state.generate.sweep.enhance_ask.take();
            state.generate = f;
            mark_generate(state);
            fx
        }
        Msg::UseImage(image, target) => use_image(state, image, target),
        Msg::ToInputLibrary(image) => {
            lib(state, LibOp::IdeaFromOutput { image: image.id, with_prompt: false, loc: IdeaLoc::top_library() })
        }
        Msg::NewIdeaFromImage(image) => {
            lib(state, LibOp::IdeaFromOutput { image: image.id, with_prompt: true, loc: IdeaLoc::Unreviewed })
        }
        Msg::ClearInit => {
            // the image stays in the list (as a reference, or parked)
            for x in &mut state.generate.images {
                x.start = false;
            }
            mark_generate(state);
            vec![]
        }
        Msg::DuplicateProject(path) => {
            let base = path.file_name().unwrap_or_default().to_string_lossy();
            let name = std::iter::once(format!("{base} copy"))
                .chain((2..).map(|n| format!("{base} copy {n}")))
                .find(|n| !state.project_name_taken(&path, n))
                .unwrap_or_default();
            state.duplicate = Some(DuplicateDialog { path, name });
            vec![]
        }
        Msg::EditDuplicateName(n) => {
            if let Some(d) = &mut state.duplicate {
                d.name = n;
            }
            vec![]
        }
        Msg::ConfirmDuplicate => {
            let Some(d) = state.duplicate.take() else { return vec![] };
            let name = d.name.trim().to_string();
            let problem = increart_core::library::validate_name(&name).err().map(|e| e.to_string());
            if let Some(e) = problem
                .or_else(|| state.project_name_taken(&d.path, &name).then(|| format!("{name} already exists here")))
            {
                state.messages_error(e);
                state.duplicate = Some(d);
                return vec![];
            }
            lib(state, LibOp::CopyProject(d.path, name))
        }
        Msg::CancelDuplicate => {
            state.duplicate = None;
            vec![]
        }
        Msg::MoreSeeds(run, n) => {
            if n == 0 {
                return vec![];
            }
            lib(state, LibOp::AddSeeds(run, n))
        }
        // ------------------------------------------------ prompt presets
        Msg::PresetsPanel(open) => {
            let favourites = state.prompt_presets.tasks.iter().flat_map(|t| &t.presets).any(|p| p.favourite);
            let view = if favourites { PresetsView::Favourites } else { PresetsView::Task(0) };
            state.presets_panel = open.then(|| PresetsPanel { view, search: String::new() });
            vec![]
        }
        Msg::PresetsShow(view) => {
            if let Some(p) = &mut state.presets_panel {
                p.view = view;
            }
            vec![]
        }
        Msg::PresetsSearch(q) => {
            if let Some(p) = &mut state.presets_panel {
                p.search = q;
            }
            vec![]
        }
        Msg::ApplyPreset { task, preset, insert_at } => {
            let Some(p) = state.prompt_presets.tasks.get(task).and_then(|t| t.presets.get(preset)).cloned() else {
                return vec![];
            };
            let mut fx = vec![];
            // the bytes of the prompt that are the preset's
            let at = match insert_at {
                Some(c) => {
                    let prompt = &mut state.generate.prompt;
                    let b = prompt.char_indices().nth(c).map_or(prompt.len(), |(i, _)| i);
                    let before = if b > 0 && !prompt[..b].ends_with(char::is_whitespace) { " " } else { "" };
                    let after =
                        if b < prompt.len() && !prompt[b..].starts_with(char::is_whitespace) { " " } else { "" };
                    prompt.insert_str(b, &format!("{before}{}{after}", p.prompt));
                    b + before.len()..b + before.len() + p.prompt.len()
                }
                None => {
                    let f = &state.generate;
                    if let Some(project) = state.project.clone()
                        && !f.prompt.trim().is_empty()
                        && f.prompt != p.prompt
                    {
                        let (t, x) = (f.prompt.clone(), f.negative.clone());
                        fx = record(state, HistoryKey::Project(project), HistoryKind::BeforePreset, t, x);
                    }
                    state.generate.prompt = p.prompt.clone();
                    if !p.negative.is_empty() {
                        state.generate.negative = p.negative.clone();
                    }
                    0..p.prompt.len()
                }
            };
            // its first {slot} selected for typing over, else the cursor after it
            let prompt = &state.generate.prompt;
            let sel = increart_core::prompts::slots(&prompt[at.clone()])
                .first()
                .map_or(at.end..at.end, |r| at.start + r.start..at.start + r.end);
            let chars = |b: usize| prompt[..b].chars().count();
            state.prompt_select = Some(chars(sel.start)..chars(sel.end));
            mark_generate(state);
            fx
        }
        Msg::StylesOpen => {
            use increart_core::prompts::{PresetKey, SweepSource, sweep_presets};
            let sw = &state.generate.sweep;
            let before = sw.clone();
            if sw.source != SweepSource::Selected {
                let whole = increart_core::prompts::Sweep { random: 0, removed: vec![], ..sw.clone() };
                let keys = sweep_presets(&state.prompt_presets, &whole)
                    .into_iter()
                    .map(|(t, p)| PresetKey { task: t.name.clone(), name: p.name.clone() })
                    .collect();
                let sw = &mut state.generate.sweep;
                sw.selected = keys;
                sw.source = SweepSource::Selected;
                sw.removed.clear();
                mark_generate(state);
            }
            let favourites = state.prompt_presets.tasks.iter().flat_map(|t| &t.presets).any(|p| p.favourite);
            let view = if !state.generate.sweep.selected.is_empty() {
                StyleView::Ticked
            } else if favourites {
                StyleView::Favourites
            } else {
                StyleView::All
            };
            let stats = state.style_stats();
            state.styles = Some(StylePicker { view, before, stats, ..Default::default() });
            vec![]
        }
        Msg::StylesShow(view) => {
            if let Some(p) = &mut state.styles {
                p.view = view;
            }
            vec![]
        }
        Msg::StylesSearch(q) => {
            if let Some(p) = &mut state.styles {
                p.search = q;
            }
            vec![]
        }
        Msg::StylesFilter(f) => {
            if let Some(p) = &mut state.styles {
                p.filter = f;
            }
            vec![]
        }
        Msg::StylesSort(sort) => {
            state.config.style_sort = sort;
            save_config(state)
        }
        Msg::StylesClose(keep) => {
            let Some(p) = state.styles.take() else { return vec![] };
            let sw = &mut state.generate.sweep;
            if !keep && *sw != p.before {
                *sw = p.before;
                mark_generate(state);
            } else if keep {
                // a random pick's replacements were of the old pick
                sw.removed.retain(|r| sw.selected.contains(&r.preset));
            }
            vec![]
        }
        Msg::PeRewrite(target) => {
            let original = match &target {
                PeTarget::Prompt => state.generate.prompt.clone(),
                PeTarget::NewIdea => state.draft.text.clone(),
                PeTarget::Idea(id) => match &state.idea_edit {
                    Some(ed) if ed.id == *id => ed.text.clone(),
                    _ => state.idea(id).map(|e| e.idea.text.clone()).unwrap_or_default(),
                },
                PeTarget::Describe(_) => String::new(),
            };
            if original.trim().is_empty() && !matches!(target, PeTarget::Describe(_)) {
                state.messages_error("Nothing to rewrite: write some text first");
                return vec![];
            }
            // the same text again keeps the instructions given
            let how = state.pe.text.as_ref().filter(|t| t.target == target).map(|t| t.how.clone()).unwrap_or_default();
            let mut fx = stop_text_ask(state);
            state.pe.text = Some(PeText {
                target,
                original,
                how,
                text: String::new(),
                answer: String::new(),
                waiting: None,
                error: None,
                images: 0,
            });
            fx.extend(ask_text(state));
            fx
        }
        Msg::PeTextEdit { how, text } => {
            if let Some(t) = &mut state.pe.text {
                (t.how, t.text) = (how, text);
            }
            vec![]
        }
        Msg::PeTextRetry => ask_text(state),
        Msg::PeTextClose(how) => {
            let mut fx = stop_text_ask(state);
            let Some(t) = state.pe.text.take() else { return fx };
            if how != PeUse::Discard && !t.text.trim().is_empty() {
                fx.extend(use_text(state, t, how));
            }
            fx
        }
        Msg::PeStyleEnhance(keys) => {
            let keys = if keys.is_empty() { styles_to_enhance(state) } else { keys };
            ask_styles(state, &keys, &EnhanceAsk::default(), 0)
        }
        Msg::PeStyleOpen => {
            if state.pe.server.is_none() {
                return vec![];
            }
            let ask = match &state.generate.sweep.enhance_ask {
                Some(a) => a.clone(),
                None => EnhanceAsk { styles: styles_to_enhance(state), ..Default::default() },
            };
            state.pe.style_dialog = Some(ask);
            vec![]
        }
        Msg::PeStyleDialog(a) => {
            if let Some(a) = &a
                && state.generate.sweep.enhance_ask.as_ref() != Some(a)
            {
                state.generate.sweep.enhance_ask = Some(a.clone());
                mark_generate(state);
            }
            state.pe.style_dialog = a;
            vec![]
        }
        Msg::PeStyleAsk => {
            let Some(a) = state.pe.style_dialog.take() else { return vec![] };
            ask_styles(state, &a.styles, &a, 0)
        }
        Msg::PeAddedBlanks(keep) => {
            state.pe.added_blanks_ask = false;
            for (key, names) in std::mem::take(&mut state.pe.added_blanks) {
                let Some(own) = state.generate.sweep.style_prompt(&key).filter(|_| !keep) else { continue };
                let prompt = increart_core::prompts::unblank(own.prompt.clone(), &names);
                let own = increart_core::prompts::StylePrompt { prompt, ..own.clone() };
                state.generate.sweep.set_style_prompt(&key, Some(own));
                mark_generate(state);
            }
            vec![]
        }
        Msg::PeSuggest(keys) => {
            let Some(server) = state.pe.server.as_ref().cloned() else {
                state.messages_error(NO_CHAT);
                return vec![];
            };
            let keys: Vec<_> = keys.into_iter().filter(|k| state.pe.blank_job(k).is_none()).collect();
            let jobs: Vec<_> = keys.iter().filter_map(|k| blank_job(state, &server, k, None)).collect();
            if jobs.is_empty() {
                return vec![];
            }
            if state.pe.blanks_waiting() == 0 {
                state.pe.blank_batch = 0;
            }
            state.pe.blank_batch += jobs.len();
            for j in &jobs {
                if let PeKey::Blank(k) = &j.key {
                    state.pe.avoiding.insert(k.clone());
                }
            }
            let jobs = jobs.into_iter().map(|j| queue_blank(state, j, None, 0)).collect();
            vec![Effect::PeAsk { servers: pool(state, server), jobs }]
        }
        Msg::PeSuggestOpen(key) => {
            if state.pe.server.is_none() {
                return vec![];
            }
            let sweep = &state.generate.sweep;
            let ask = sweep.blank_values(key.style.as_ref(), &key.slot).and_then(|v| v.ask.clone()).unwrap_or_default();
            let form = SuggestForm::from_ask(&ask, state.config.pe.sampling(increart_core::pe::FILL_BLANKS));
            state.pe.suggest = Some(SuggestDialog { key, form });
            vec![]
        }
        Msg::PeSuggestDialog(d) => {
            if let Some(d) = &d {
                let ask = d.form.to_ask(state.config.pe.sampling(increart_core::pe::FILL_BLANKS));
                let ask = Some(ask).filter(|a| *a != SuggestAsk::default());
                let sweep = &mut state.generate.sweep;
                if sweep.blank_values(d.key.style.as_ref(), &d.key.slot).and_then(|v| v.ask.as_ref()) != ask.as_ref() {
                    sweep.edit_blank_values(d.key.style.as_ref(), &d.key.slot, |v| v.ask = ask);
                    mark_generate(state);
                }
            }
            state.pe.suggest = d;
            vec![]
        }
        Msg::PeBatchSuggestOpen(ticked) => {
            if state.pe.server.is_none() {
                return vec![];
            }
            let b = state.generate.sweep.batch_ask.clone().unwrap_or(BatchAsk { blanks: ticked, ..Default::default() });
            let form = SuggestForm::from_ask(&b.ask, state.config.pe.sampling(increart_core::pe::FILL_BLANKS));
            state.pe.batch_suggest = Some(BatchSuggest { blanks: b.blanks, form });
            vec![]
        }
        Msg::PeBatchSuggestDialog(b) => {
            if let Some(b) = &b {
                let ask = b.form.to_ask(state.config.pe.sampling(increart_core::pe::FILL_BLANKS));
                let ask = Some(BatchAsk { ask, blanks: b.blanks.clone() });
                if state.generate.sweep.batch_ask != ask {
                    state.generate.sweep.batch_ask = ask;
                    mark_generate(state);
                }
            }
            state.pe.batch_suggest = b;
            vec![]
        }
        Msg::PeBatchSuggestAsk => {
            let (Some(server), Some(b)) = (state.pe.server.as_ref().cloned(), state.pe.batch_suggest.take()) else {
                return vec![];
            };
            let keys = b.blanks.iter().map(|slot| BlankKey { style: None, slot: slot.clone() });
            let keys: Vec<_> = keys.filter(|k| state.pe.blank_job(k).is_none()).collect();
            let jobs: Vec<_> = keys.iter().filter_map(|k| blank_job(state, &server, k, Some(&b.form))).collect();
            if jobs.is_empty() {
                return vec![];
            }
            if state.pe.blanks_waiting() == 0 {
                state.pe.blank_batch = 0;
            }
            state.pe.blank_batch += jobs.len();
            for j in &jobs {
                if let PeKey::Blank(k) = &j.key {
                    if b.form.replace {
                        state.pe.rerolls.insert(k.clone());
                    }
                    if b.form.avoid {
                        state.pe.avoiding.insert(k.clone());
                    }
                }
            }
            let jobs = jobs.into_iter().map(|j| queue_blank(state, j, Some(&b.form), 0)).collect();
            vec![Effect::PeAsk { servers: pool(state, server), jobs }]
        }
        Msg::PeSuggestAsk => {
            let (Some(server), Some(d)) = (state.pe.server.as_ref().cloned(), state.pe.suggest.clone()) else {
                return vec![];
            };
            if state.pe.blank_job(&d.key).is_some() {
                return vec![];
            }
            let Some(job) = blank_job(state, &server, &d.key, Some(&d.form)) else { return vec![] };
            if d.form.replace {
                state.pe.rerolls.insert(d.key.clone());
            }
            if d.form.avoid {
                state.pe.avoiding.insert(d.key.clone());
            }
            if state.pe.blanks_waiting() == 0 {
                state.pe.blank_batch = 0;
            }
            state.pe.blank_batch += 1;
            let job = queue_blank(state, job, Some(&d.form), 0);
            vec![Effect::PeAsk { servers: pool(state, server), jobs: vec![job] }]
        }
        Msg::PeForgetOffered(key) => {
            state.generate.sweep.edit_blank_values(key.style.as_ref(), &key.slot, |v| v.offered.clear());
            mark_generate(state);
            vec![]
        }
        Msg::PeClearSuggestions(key) => {
            state.generate.sweep.edit_blank_values(key.style.as_ref(), &key.slot, |v| v.shown.clear());
            mark_generate(state);
            vec![]
        }
        Msg::PeReroll(key) => {
            if state.pe.blank_job(&key).is_some() {
                return vec![];
            }
            state.pe.rerolls.insert(key.clone());
            let fx = update(state, Msg::PeSuggest(vec![key.clone()]));
            if state.pe.blank_job(&key).is_none() {
                state.pe.rerolls.remove(&key);
            }
            fx
        }
        Msg::PeStatus(s) => {
            match &s {
                PeStatus::Starting => state.pe.starting_since = state.now,
                PeStatus::Ready if state.pe.status == PeStatus::Starting => {
                    state.pe.last_load = Some(state.now - state.pe.starting_since)
                }
                _ => {}
            }
            if let PeStatus::Failed(e) = &s {
                state.messages_error(e.clone());
                state.pe.jobs.clear();
                state.pe.blank_tries.clear();
                state.pe.style_tries.clear();
                if let Some(t) = &mut state.pe.text
                    && t.waiting.take().is_some()
                {
                    t.error = Some(e.clone());
                }
            }
            state.pe.status = s;
            vec![]
        }
        Msg::PeReady(n) => {
            state.pe.ready = n;
            vec![]
        }
        Msg::PeOn(id, name) => {
            if let Some(j) = state.pe.jobs.iter_mut().find(|j| j.id == id) {
                j.on = Some(name);
            }
            vec![]
        }
        Msg::PeProgress(id, step) => {
            let now = state.now;
            let Some(j) = state.pe.jobs.iter_mut().find(|j| j.id == id) else { return vec![] };
            if std::mem::discriminant(&j.step) != std::mem::discriminant(&step) {
                if let (PeStep::Reading { images }, PeStep::Writing { .. }) = (&j.step, &step) {
                    state.pe.read_times.insert(*images, now - j.since);
                }
                j.since = now;
                j.rate = None;
            }
            if let PeStep::Writing { pieces, .. } = &step
                && now - j.since >= 1.0
                && j.rate.is_none_or(|(at, _)| now - at >= RATE_EVERY)
            {
                j.rate = Some((now, *pieces as f64 / (now - j.since)));
            }
            j.step = step;
            vec![]
        }
        Msg::PeStop(ids) => {
            for j in state.pe.jobs.iter().filter(|j| ids.contains(&j.id)) {
                match &j.key {
                    PeKey::Blank(k) => {
                        state.pe.rerolls.remove(k);
                        state.pe.avoiding.remove(k);
                        state.pe.blank_tries.remove(k);
                    }
                    PeKey::Style(k) => {
                        state.pe.style_from.remove(k);
                        state.pe.style_tries.remove(k);
                    }
                    PeKey::Text(_) => {}
                }
            }
            state.pe.jobs.retain(|j| !ids.contains(&j.id));
            ask_added_blanks(state);
            if let Some(t) = &mut state.pe.text
                && t.waiting.is_some_and(|a| ids.contains(&a))
            {
                t.waiting = None;
                t.error = Some("Stopped".into());
            }
            vec![Effect::PeStop(ids)]
        }
        Msg::PeAnswer(id, r) => {
            // an ask stopped (or replaced) has no answer
            let Some(i) = state.pe.jobs.iter().position(|j| j.id == id) else { return vec![] };
            let job = state.pe.jobs.remove(i);
            match job.key {
                PeKey::Blank(key) => {
                    let reroll = state.pe.rerolls.remove(&key);
                    let avoid = state.pe.avoiding.remove(&key);
                    let tried = state.pe.blank_tries.remove(&key).unwrap_or_default();
                    // nothing new: asked again (hotter), else why
                    let failed = match r.map(|a| increart_core::pe::parse_suggestions(&a)) {
                        Ok(v) if !v.is_empty() => {
                            use increart_core::pe::same_suggestion;
                            // small models repeat themselves despite being told
                            // (unless the dialog said they may)
                            let offered = state.blank_offered(&key);
                            let fresh: Vec<String> = if avoid {
                                v.into_iter().filter(|x| !offered.iter().any(|o| same_suggestion(o, x))).collect()
                            } else {
                                v
                            };
                            if fresh.is_empty() {
                                Some((true, "only repeated values offered before".to_string()))
                            } else {
                                let add = |list: &mut Vec<String>, x: &String| {
                                    if !list.iter().any(|o| o.eq_ignore_ascii_case(x)) {
                                        list.push(x.clone());
                                    }
                                };
                                state.generate.sweep.edit_blank_values(key.style.as_ref(), &key.slot, |v| {
                                    // a reroll replaces them; else they're added to
                                    if reroll {
                                        v.shown.clear();
                                    }
                                    for x in &fresh {
                                        add(&mut v.offered, x);
                                        add(&mut v.shown, x);
                                    }
                                });
                                mark_generate(state);
                                None
                            }
                        }
                        Ok(_) => Some((true, "offered nothing (or nothing readable)".to_string())),
                        Err(e) => Some((false, e)),
                    };
                    let Some((retry, why)) = failed else { return vec![] };
                    if retry
                        && tried.retry < BLANK_RETRIES
                        && let Some(server) = state.pe.server.clone()
                    {
                        let fill = state.config.pe.sampling(increart_core::pe::FILL_BLANKS);
                        let mut form =
                            tried.form.unwrap_or_else(|| SuggestForm::from_ask(&SuggestAsk::default(), fill));
                        form.sampling.temperature = (form.sampling.temperature + RETRY_HOTTER).min(2.0);
                        if !form.how.contains(RETRY_NOTE) {
                            form.how = [form.how.trim(), RETRY_NOTE].join(" ").trim().to_string();
                        }
                        if let Some(job) = blank_job(state, &server, &key, Some(&form)) {
                            if reroll {
                                state.pe.rerolls.insert(key.clone());
                            }
                            if avoid {
                                state.pe.avoiding.insert(key.clone());
                            }
                            state.pe.blank_batch += 1;
                            let job = queue_blank(state, job, Some(&form), tried.retry + 1);
                            return vec![Effect::PeAsk { servers: pool(state, server), jobs: vec![job] }];
                        }
                    }
                    let asked = if retry { format!(", asked {} times", tried.retry + 1) } else { String::new() };
                    state.messages_error(format!(
                        "PE {why} for {{{}}}{asked}: try Suggest… hotter, or with instructions",
                        key.slot
                    ));
                    state.pe.blank_failed.insert(key, format!("{why}{asked}"));
                }
                PeKey::Style(key) => {
                    let from = state.pe.style_from.remove(&key).unwrap_or_default();
                    let tried = state.pe.style_tries.remove(&key);
                    let why = match r.map(|a| increart_core::pe::clean_text(&a)) {
                        Ok(prompt) if prompt.is_empty() => Some("answered nothing".to_string()),
                        Ok(prompt) if tried.as_ref().is_some_and(|t| t.sent.trim() == prompt.trim()) => {
                            Some("gave the prompt back unchanged".to_string())
                        }
                        Ok(prompt) => {
                            let added = increart_core::prompts::added_slots(&from, &prompt);
                            if !added.is_empty() {
                                state.pe.added_blanks.push((key.clone(), added));
                            }
                            let p = increart_core::prompts::StylePrompt {
                                preset: key.clone(),
                                prompt,
                                from,
                                enhanced: true,
                            };
                            state.generate.sweep.set_style_prompt(&key, Some(p));
                            mark_generate(state);
                            None
                        }
                        Err(e) => {
                            state.messages_error(format!("PE, {}: {e}", key.name));
                            state.pe.style_failed.insert(key.clone(), e);
                            None
                        }
                    };
                    if let Some(why) = why {
                        let tried = tried.unwrap_or_else(|| StyleTry {
                            ask: EnhanceAsk::default(),
                            sent: String::new(),
                            retry: 0,
                        });
                        if tried.retry < STYLE_RETRIES {
                            let mut a = tried.ask.clone();
                            let usual = state.config.pe.sampling(increart_core::pe::STYLE_PROMPT);
                            let mut x = a.sampling.unwrap_or(usual);
                            x.temperature = (x.temperature + RETRY_HOTTER).min(2.0);
                            a.sampling = Some(x);
                            if !a.how.contains(STYLE_RETRY_NOTE) {
                                a.how = [a.how.trim(), STYLE_RETRY_NOTE].join(" ").trim().to_string();
                            }
                            let fx = ask_styles(state, std::slice::from_ref(&key), &a, tried.retry + 1);
                            if !fx.is_empty() {
                                ask_added_blanks(state);
                                return fx;
                            }
                        }
                        let asked = format!(", asked {} times", tried.retry + 1);
                        state.messages_error(format!(
                            "PE {why} for {}{asked}: try Enhance… with instructions",
                            key.name
                        ));
                        state.pe.style_failed.insert(key, format!("{why}{asked}"));
                    }
                }
                PeKey::Text(ask) => {
                    let Some(t) = state.pe.text.as_mut().filter(|t| t.waiting == Some(ask)) else { return vec![] };
                    t.waiting = None;
                    match r.map(|a| increart_core::pe::clean_text(&a)) {
                        Ok(a) if !a.is_empty() => (t.text, t.answer) = (a.clone(), a),
                        Ok(_) => t.error = Some("The server answered with nothing".into()),
                        Err(e) => t.error = Some(e),
                    }
                }
            }
            ask_added_blanks(state);
            vec![]
        }
        // (one running as asked is kept)
        Msg::PeLaunch => match state.pe.server.as_ref() {
            Some(s) if s.start == increart_core::pe::PeStart::AtLaunch => update(state, Msg::PeStart),
            _ => vec![],
        },
        Msg::PeStart => {
            let Some(server) = state.pe.server.as_ref().cloned() else { return vec![] };
            // an ask with nothing to ask: the server is started (after a
            // stop asked before it; one running as asked is kept)
            vec![Effect::PeAsk { servers: pool(state, server), jobs: vec![] }]
        }
        Msg::PeShutdown => {
            let ids: Vec<u64> = state.pe.jobs.iter().map(|j| j.id).collect();
            let mut fx = if ids.is_empty() { vec![] } else { update(state, Msg::PeStop(ids)) };
            if state.pe.status != PeStatus::Off {
                fx.push(Effect::PeShutdown);
            }
            fx
        }
        Msg::PeEditorOpen(index) => {
            let Phase::Servers(d) = &state.phase else { return vec![] };
            let draft = match index.and_then(|i| d.draft.servers.get(i)) {
                Some(ServerConfig::Chat(s)) => s.clone(),
                Some(_) => return vec![],
                // a new one: the presets' first, else KoboldCpp on the GPUs
                None => state
                    .config
                    .chat_servers()
                    .first()
                    .map(|s| (*s).clone())
                    .unwrap_or_else(|| new_pe_server(state.gpu_runtime)),
            };
            let mut draft = draft;
            // another beside ones the preset has: its own name and port
            if index.is_none() && d.draft.chat().is_some() {
                let n = d.draft.servers.iter().filter(|s| s.is_chat()).count() + 1;
                draft.name = format!("{} {n}", draft.name.trim_end_matches(|c: char| c.is_ascii_digit() || c == ' '));
                let used = d.draft.servers.iter().filter_map(ServerConfig::port).max();
                if let (PeRun::KoboldCpp { port, .. }, Some(used)) = (&mut draft.run, used) {
                    *port = (*port).max(used + 1);
                }
            }
            state.pe.editor = Some(PeEditor { index, draft, error: None });
            list_gpus_for_editor(state)
        }
        Msg::PeEditorEdit(draft) => {
            if let Some(e) = &mut state.pe.editor {
                e.draft = draft;
                e.error = None;
            }
            list_gpus_for_editor(state)
        }
        Msg::PeEditorClose => {
            state.pe.editor = None;
            vec![]
        }
        Msg::PeEditorSave => {
            let Some(e) = state.pe.editor.clone() else { return vec![] };
            let name = e.draft.name.trim().to_string();
            let problem = if name.is_empty() {
                Some("It needs a name.".to_string())
            } else {
                match &e.draft.run {
                    PeRun::KoboldCpp { binary, model, config, .. }
                        if binary.trim().is_empty() || (model.trim().is_empty() && config.trim().is_empty()) =>
                    {
                        Some("KoboldCpp needs its binary, and a model or a settings file.".into())
                    }
                    PeRun::Connect { url } if url.trim().is_empty() => Some("It needs the server's address.".into()),
                    _ => None,
                }
            };
            if let Some(p) = problem {
                if let Some(ed) = &mut state.pe.editor {
                    ed.error = Some(p);
                }
                return vec![];
            }
            let Phase::Servers(d) = &mut state.phase else { return vec![] };
            let mut server = ServerConfig::Chat(increart_core::pe::PeServer { name, ..e.draft });
            server.full_paths(&mut vec![]);
            // a new one is added: prompt enhance shares its asks among them
            let at = e.index.filter(|i| d.draft.servers.get(*i).is_some_and(ServerConfig::is_chat));
            match at {
                Some(i) => d.draft.servers[i] = server,
                None => d.draft.servers.push(server),
            }
            d.progress.clear();
            state.pe.editor = None;
            vec![]
        }
        Msg::PeListGpus(runtime) => {
            state.pe.gpus = None;
            state.pe.gpus_runtime = runtime;
            vec![Effect::PeListGpus(runtime)]
        }
        Msg::PeGpus(runtime, r) => {
            // (a stale one: another runtime asked for since)
            if runtime == state.pe.gpus_runtime {
                state.pe.gpus = Some(r);
            }
            vec![]
        }
        Msg::PeSampling { job, sampling } => {
            match sampling {
                Some(x) => _ = state.config.pe.sampling.insert(job, x),
                None => _ = state.config.pe.sampling.remove(&job),
            }
            save_config(state)
        }
        Msg::PeSystemPrompt { job, text } => {
            if text.trim().is_empty() {
                state.config.pe.system_prompts.remove(&job);
            } else {
                state.config.pe.system_prompts.insert(job, text);
            }
            save_config(state)
        }
        Msg::PromptSelected => {
            state.prompt_select = None;
            vec![]
        }
        Msg::SweepToggle(t, p) => {
            let Some((task, preset)) = state.prompt_presets.tasks.get(t).and_then(|x| Some((x, x.presets.get(p)?)))
            else {
                return vec![];
            };
            let key = increart_core::prompts::PresetKey { task: task.name.clone(), name: preset.name.clone() };
            update(state, Msg::SweepToggleKey(key))
        }
        Msg::SweepToggleKey(key) => {
            let sw = &mut state.generate.sweep;
            match sw.selected.iter().position(|k| *k == key) {
                Some(i) => {
                    sw.selected.remove(i);
                }
                None => sw.selected.push(key),
            }
            sw.source = increart_core::prompts::SweepSource::Selected;
            mark_generate(state);
            vec![]
        }
        Msg::SweepTicks(presets, on) => {
            let keys = presets.into_iter().filter_map(|(t, p)| {
                let task = state.prompt_presets.tasks.get(t)?;
                Some(increart_core::prompts::PresetKey {
                    task: task.name.clone(),
                    name: task.presets.get(p)?.name.clone(),
                })
            });
            let keys: Vec<_> = keys.collect();
            let sw = &mut state.generate.sweep;
            if on {
                for k in keys {
                    if !sw.selected.contains(&k) {
                        sw.selected.push(k);
                    }
                }
            } else {
                sw.selected.retain(|k| !keys.contains(k));
            }
            sw.source = increart_core::prompts::SweepSource::Selected;
            mark_generate(state);
            vec![]
        }
        Msg::SweepClearTicks => {
            state.generate.sweep.selected.clear();
            mark_generate(state);
            vec![]
        }
        Msg::PresetFavourite(t, p) => {
            let Some(x) = state.prompt_presets.tasks.get_mut(t).and_then(|t| t.presets.get_mut(p)) else {
                return vec![];
            };
            x.favourite = !x.favourite;
            save_presets(state)
        }
        Msg::OpenPresetEditor(task, preset) => {
            let list = state.prompt_presets.clone();
            let task = task.min(list.tasks.len().saturating_sub(1));
            state.presets_editor = Some(PresetsEditor { list, task, preset, error: None });
            vec![]
        }
        Msg::EditPresetEditor(e) => {
            if state.presets_editor.is_some() {
                state.presets_editor = Some(e);
            }
            vec![]
        }
        Msg::RestorePresetDefaults => {
            if let Some(e) = &mut state.presets_editor {
                *e = PresetsEditor { list: increart_core::prompts::builtin(), task: 0, preset: None, error: None };
            }
            vec![]
        }
        Msg::SavePresetEditor => {
            let Some(e) = &mut state.presets_editor else { return vec![] };
            if let Err(err) = check_presets(&e.list) {
                e.error = Some(err);
                return vec![];
            }
            let Some(e) = state.presets_editor.take() else { return vec![] };
            state.prompt_presets = e.list.normalized();
            let n = state.prompt_presets.tasks.len();
            if let Some(p) = &mut state.presets_panel
                && p.view.task().is_some_and(|t| t >= n)
            {
                p.view = if n > 0 { PresetsView::Task(0) } else { PresetsView::Favourites };
            }
            // a group edited away: its task
            if let Some(p) = &mut state.presets_panel
                && let PresetsView::Group(t, g) = &p.view
                && !state.prompt_presets.tasks[*t].presets.iter().any(|x| &x.group == g)
            {
                p.view = PresetsView::Task(*t);
            }
            save_presets(state)
        }
        Msg::ClosePresetEditor => {
            state.presets_editor = None;
            vec![]
        }
        Msg::OpenSavePreset => {
            let f = &state.generate;
            if f.prompt.trim().is_empty() {
                state.messages_error("The prompt is empty");
                return vec![];
            }
            let view = state.presets_panel.as_ref().map(|p| &p.view);
            let task = view.and_then(|v| v.task()).unwrap_or(0);
            // into the group shown, if one is
            let group = view.and_then(|v| v.group()).unwrap_or_default().to_string();
            let preset =
                PromptPreset { prompt: f.prompt.clone(), negative: f.negative.clone(), group, ..Default::default() };
            state.save_preset = Some(SavePresetDialog { task, preset, error: None });
            vec![]
        }
        Msg::EditSavePreset(d) => {
            if state.save_preset.is_some() {
                state.save_preset = Some(d);
            }
            vec![]
        }
        Msg::ConfirmSavePreset => {
            let Some(d) = &mut state.save_preset else { return vec![] };
            let name = d.preset.name.trim().to_string();
            let Some(task) = state.prompt_presets.tasks.get(d.task) else {
                d.error = Some("Pick a task".into());
                return vec![];
            };
            if name.is_empty() {
                d.error = Some("Give it a name".into());
                return vec![];
            }
            if task.presets.iter().any(|p| p.name.eq_ignore_ascii_case(&name)) {
                d.error = Some(format!("{} already has a preset called {name}", task.name));
                return vec![];
            }
            if let Some(why) = increart_core::prompts::slot_rule(&task.name, &d.preset.prompt) {
                d.error = Some(format!("A preset in {} {why}", task.name));
                return vec![];
            }
            let Some(mut d) = state.save_preset.take() else { return vec![] };
            d.preset.name = name.clone();
            let task = &mut state.prompt_presets.tasks[d.task];
            task.presets.push(d.preset);
            let what = format!("Saved the preset {name} in {}", task.name);
            state.prompt_presets = state.prompt_presets.normalized();
            state.messages_info(what);
            save_presets(state)
        }
        Msg::CancelSavePreset => {
            state.save_preset = None;
            vec![]
        }
        Msg::OpenInGenerate(id) => {
            let fx = update(state, Msg::SelectProject(id));
            state.tab = Tab::Projects;
            state.projects_tab = ProjectsTab::Generate;
            fx
        }
        // ------------------------------------------------ prompt history
        Msg::OpenHistory(key) => {
            state.history_view = Some(HistoryView::new(key.clone()));
            vec![Effect::LoadHistory(key)]
        }
        Msg::HistoryLoaded(key, mut entries) => {
            let mut fx = vec![];
            if let Some(ask) = state.new_sweep.take_if(|a| key == HistoryKey::Project(a.project.clone())) {
                fx = new_sweep(state, ask, &entries);
            }
            let Some(mut v) = state.history_view.take() else { return fx };
            if v.key == key {
                entries.reverse();
                v.entries = entries;
                v.selected = 0;
                resolve_history_runs(state, &mut v);
                // Show in history: the run's checkpoint, among what led to it
                if let Some((run, _)) = v.run_filter.clone() {
                    v.run_filter = Some((run.clone(), run_before(state, &v, &run)));
                    if let Some(i) = (0..v.entries.len()).find(|i| v.runs_of(*i).contains(&run)) {
                        v.selected = i;
                    }
                }
            }
            state.history_view = Some(v);
            fx
        }
        Msg::HistorySettingsOnly(on) => {
            if let Some(v) = &mut state.history_view {
                v.settings_only = on;
            }
            vec![]
        }
        Msg::HistoryRunFilter(run) => {
            let Some(mut v) = state.history_view.take() else { return vec![] };
            v.run_filter = run.map(|r| (r.clone(), run_before(state, &v, &r)));
            state.history_view = Some(v);
            vec![]
        }
        Msg::RunHistory(run) => {
            let Some(p) = state.index.run(&run).map(|(p, _)| p.project.id.clone()) else { return vec![] };
            let fx = update(state, Msg::OpenHistory(HistoryKey::Project(p)));
            if let Some(v) = &mut state.history_view {
                v.run_filter = Some((run, None));
            }
            fx
        }
        Msg::HistorySelect(i) => {
            if let Some(v) = &mut state.history_view {
                v.selected = i;
            }
            vec![]
        }
        Msg::HistorySearch(q) => {
            if let Some(v) = &mut state.history_view {
                v.search = q;
            }
            vec![]
        }
        Msg::HistoryCheckpointsOnly(on) => {
            if let Some(v) = &mut state.history_view {
                v.checkpoints_only = on;
            }
            vec![]
        }
        Msg::HistoryEveryEntry(on) => {
            if let Some(v) = &mut state.history_view {
                v.every_entry = on;
            }
            vec![]
        }
        Msg::HistoryRestore(what) => {
            let Some(v) = &state.history_view else { return vec![] };
            let Some(e) = v.entries.get(v.selected).cloned() else { return vec![] };
            let key = v.key.clone();
            let with_extra = what != Restore::Text;
            let mut fx = vec![];
            match &key {
                HistoryKey::Project(p) if state.index.project(p).is_some() => {
                    // e.g. opened from a run in the Output library
                    if state.project.as_ref() != Some(p) || state.projects_tab != ProjectsTab::Generate {
                        fx.extend(update(state, Msg::OpenInGenerate(p.clone())));
                    }
                    let (t, x) = (state.generate.prompt.clone(), state.generate.negative.clone());
                    fx.extend(record(state, key.clone(), HistoryKind::BeforeRestore, t, x));
                    match (what, &e.form) {
                        (Restore::All, Some(d)) => restore_form(state, d),
                        _ => {
                            state.generate.prompt = e.text;
                            if with_extra {
                                state.generate.negative = e.extra;
                            }
                        }
                    }
                    mark_generate(state);
                }
                HistoryKey::NewIdea => {
                    let (t, x) = (state.draft.text.clone(), state.draft.title.clone());
                    fx.extend(record(state, key.clone(), HistoryKind::BeforeRestore, t, x));
                    state.draft.text = e.text;
                    if with_extra {
                        state.draft.title = e.extra;
                    }
                    state.idea_dirty = Some(state.now);
                }
                HistoryKey::Idea(id) if state.idea_edit.as_ref().is_some_and(|ed| &ed.id == id) => {
                    let Some(ed) = state.idea_edit.as_mut() else { return fx };
                    let (t, x) = (ed.text.clone(), ed.title.clone());
                    ed.text = e.text;
                    if with_extra {
                        ed.title = e.extra;
                    }
                    fx.extend(record(state, key.clone(), HistoryKind::BeforeRestore, t, x));
                }
                _ => {
                    state.messages_error("Open what this history belongs to first");
                    return vec![];
                }
            }
            state.history_view = None;
            fx
        }
        Msg::CloseHistory => {
            state.history_view = None;
            vec![]
        }
        // ------------------------------------------------ Edit idea
        Msg::OpenIdea(id) => {
            let fx = flush_idea_edit(state);
            if let Some(i) = state.idea(&id).map(|e| e.idea.clone()) {
                state.idea_edit = Some(IdeaEdit {
                    id: id.clone(),
                    title: i.title.clone(),
                    text: i.text.clone(),
                    tags: i.tags.join(", "),
                    images: i.images.clone(),
                });
                // so the first pause records against what was there
                let (t, x) = (i.text.clone(), i.title.clone());
                state.history_last.entry(HistoryKey::Idea(id)).or_insert(LastEntry {
                    at: f64::NEG_INFINITY,
                    entry: HistoryEntry::text(Default::default(), HistoryKind::Saved, t, x),
                });
            }
            fx
        }
        Msg::EditIdea(ed) => {
            if state.idea_edit.as_ref() != Some(&ed) {
                state.idea_edit = Some(ed);
                state.idea_edit_dirty = Some(state.now);
            }
            vec![]
        }
        Msg::SaveIdea => {
            let Some(ed) = state.idea_edit.clone() else { return vec![] };
            state.idea_edit_dirty = None;
            let tags = IdeaDraft { tags: ed.tags.clone(), ..Default::default() }.tag_list();
            let mut fx =
                record(state, HistoryKey::Idea(ed.id.clone()), HistoryKind::Saved, ed.text.clone(), ed.title.clone());
            state.idea_edit = None;
            fx.extend(lib(
                state,
                LibOp::SaveIdea { id: ed.id, title: ed.title, text: ed.text, tags, images: ed.images },
            ));
            fx
        }
        Msg::CloseIdea => {
            let fx = flush_idea_edit(state);
            state.idea_edit = None;
            fx
        }
        // ------------------------------------------------ input library
        Msg::LibTab(t) => {
            state.lib_tab = t;
            fv_sync(state);
            relist_if_stale(state)
        }
        Msg::LibFilter(f) => {
            state.lib_filter = f;
            vec![]
        }
        // ------------------------------------------------ Import tab
        Msg::LoadImports => vec![Effect::LoadImports],
        Msg::Imports(list) => {
            state.imports = Some(list);
            vec![]
        }
        Msg::ImportDropNow => lib(state, LibOp::ImportInboxDrop),
        // ------------------------------------------------ resolutions
        Msg::OpenResolutions => {
            state.res_editor = Some(state.config.resolutions.clone());
            vec![]
        }
        Msg::EditResolutions(list) => {
            state.res_editor = Some(list);
            vec![]
        }
        Msg::SaveResolutions => {
            let Some(list) = state.res_editor.take() else { return vec![] };
            state.config.resolutions = list;
            for w in state.config.normalize() {
                state.messages_info(w);
            }
            save_config(state)
        }
        Msg::CloseResolutions => {
            state.res_editor = None;
            vec![]
        }
        // ------------------------------------------------ model presets
        Msg::OpenRecipes => {
            state.recipe_editor = Some(RecipeEditor::new(&state.config.recipes, state.generate.preset));
            vec![]
        }
        Msg::EditRecipes(e) => {
            state.recipe_editor = Some(e);
            vec![]
        }
        Msg::SaveRecipes => {
            let Some(e) = state.recipe_editor.take() else { return vec![] };
            if let Some(p) = e.problem() {
                state.messages_error(p);
                state.recipe_editor = Some(e);
                return vec![];
            }
            let old = std::mem::take(&mut state.config.recipes);
            let mut rows = e.rows;
            for r in &mut rows {
                r.preset.name = r.preset.name.trim().to_string();
                r.preset.recipe = r.preset.recipe.trim().to_string();
                r.preset.custom_sigmas = parse_sigmas(&r.sigmas).unwrap_or_default();
            }
            // a model sweep's ticks follow renames; deleted ones go
            let renamed = |name: &str| {
                let i = old.iter().position(|p| p.name == name)?;
                rows.iter().find(|r| r.origin == Some(i)).map(|r| r.preset.name.clone())
            };
            let f = &mut state.generate;
            f.sweep.models = f.sweep.models.iter().filter_map(|m| renamed(m)).collect();
            let chosen = rows.iter().position(|r| r.origin == Some(f.preset));
            state.config.recipes = rows.into_iter().map(|r| r.preset).collect();
            let (i, changed) = match chosen {
                Some(i) => (i, old.get(state.generate.preset) != state.config.recipes.get(i)),
                None => (0, true),
            };
            state.generate.preset = i;
            // the form takes an edited preset's settings
            if changed && let Some(p) = state.config.recipes.get(i).cloned() {
                state.generate.apply_preset(i, &p);
            }
            mark_generate(state);
            save_config(state)
        }
        Msg::CloseRecipes => {
            state.recipe_editor = None;
            vec![]
        }
        Msg::OpenScale(target) => {
            let size = match target {
                ScaleTarget::Generate => Some((state.generate.width, state.generate.height)),
                ScaleTarget::Resolution(i) => {
                    state.res_editor.as_ref().and_then(|l| l.get(i)).map(|r| (r.width, r.height))
                }
            };
            state.scale = size.map(|(w, h)| ScaleDialog::new(target, w, h));
            vec![]
        }
        Msg::EditScale(d) => {
            state.scale = Some(d);
            vec![]
        }
        Msg::ApplyScale => {
            let Some(d) = state.scale.take() else { return vec![] };
            let (w, h) = d.result();
            match d.target {
                ScaleTarget::Generate => {
                    (state.generate.width, state.generate.height) = (w, h);
                    mark_generate(state);
                }
                ScaleTarget::Resolution(i) => {
                    if let Some(r) = state.res_editor.as_mut().and_then(|l| l.get_mut(i)) {
                        (r.width, r.height) = (w, h);
                    }
                }
            }
            vec![]
        }
        Msg::CloseScale => {
            state.scale = None;
            vec![]
        }
        Msg::EditNewProjectName(n) => {
            if let Some(d) = &mut state.new_project {
                d.name = n;
            }
            vec![]
        }
        Msg::ConfirmNewProject => {
            let Some(d) = state.new_project.take() else { return vec![] };
            let name = d.name.trim().to_string();
            if let Err(e) = increart_core::library::validate_name(&name) {
                state.messages_error(e.to_string());
                state.new_project = Some(d);
                return vec![];
            }
            prepare_project(state, d.image, d.target, name)
        }
        Msg::StartProject(true) => {
            if state.library.is_none() {
                return vec![];
            }
            let mut name = "new project".to_string();
            for n in 2.. {
                if !state.project_name_taken(std::path::Path::new("_"), &name) {
                    break;
                }
                name = format!("new project {n}");
            }
            state.start_project = Some(StartProjectDialog { name, refs: StartRefs::Library });
            vec![]
        }
        Msg::StartProject(false) => {
            state.start_project = None;
            vec![]
        }
        Msg::EditStartProject(d) => {
            if state.start_project.is_some() {
                state.start_project = Some(d);
            }
            vec![]
        }
        Msg::ConfirmStartProject => {
            let Some(d) = state.start_project.take() else { return vec![] };
            let name = d.name.trim().to_string();
            if increart_core::library::validate_name(&name).is_err() {
                state.start_project = Some(d);
                return vec![];
            }
            let mut form = GenerateForm::new(&state.config);
            form.label = "r00".into();
            lib(state, LibOp::PrepareProject { parent: PathBuf::new(), name, copy_of: None, form, then: d.refs })
        }
        Msg::CancelNewProject => {
            state.new_project = None;
            vec![]
        }
        Msg::Preview(image) => {
            state.preview = image;
            vec![]
        }
        Msg::PreviewStep(d) => {
            let Some((i, n)) = state.preview_position() else { return vec![] };
            let j = (i as isize + d).clamp(0, n as isize - 1) as usize;
            let img = state.preview_list().and_then(|l| l.get(j).map(|o| o.image.clone()));
            if j == i || img.is_none() {
                return vec![];
            }
            state.preview = img.clone();
            // the selection follows, so closing leaves you on this image
            if state.tab == Tab::Review && state.review_view == ReviewView::Sheet {
                let at = state.sheet().and_then(|sh| {
                    sh.rows.iter().enumerate().find_map(|(r, row)| {
                        let c = row
                            .cells
                            .iter()
                            .position(|c| matches!(c, SheetCell::Image(o) if Some(&o.image) == img.as_ref()))?;
                        Some((r, c))
                    })
                });
                if let Some(at) = at {
                    state.sheet_cell = at;
                }
                vec![]
            } else if state.tab == Tab::Review {
                state.review_cursor = j;
                unfollow(state);
                vec![]
            } else {
                update(state, Msg::OutputSelect(img))
            }
        }
        Msg::PreviewRow(d) => {
            if state.tab != Tab::Review || state.review_view != ReviewView::Sheet || d == 0 {
                return vec![];
            }
            let Some(sh) = state.sheet() else { return vec![] };
            let image = |r: usize, c: usize| match sh.rows.get(r)?.cells.get(c)? {
                SheetCell::Image(o) => Some(o.image.clone()),
                _ => None,
            };
            let at = sh.rows.iter().enumerate().find_map(|(r, row)| {
                let c = row
                    .cells
                    .iter()
                    .position(|c| matches!(c, SheetCell::Image(o) if Some(&o.image) == state.preview.as_ref()))?;
                Some((r, c))
            });
            let Some((r, c)) = at else { return vec![] };
            let mut to = r as isize + d.signum();
            while to >= 0 && (to as usize) < sh.rows.len() {
                if let Some(img) = image(to as usize, c) {
                    state.preview = Some(img);
                    state.sheet_cell = (to as usize, c);
                    break;
                }
                to += d.signum();
            }
            vec![]
        }
        Msg::PreviewRate(v) => {
            let Some((i, n)) = state.preview_position() else { return vec![] };
            let Some(list) = state.preview_list() else { return vec![] };
            let link = list[i].path.clone();
            let queued = state.tab == Tab::Review && matches!(state.review_view, ReviewView::Single | ReviewView::Grid);
            // the queue's next image once this one has left it
            let next = if i + 1 < n { Some(i + 1) } else { i.checked_sub(1) };
            let next = next.map(|j| list[j].image.clone());
            let fx = lib(state, LibOp::ReviewOutputs { links: vec![(link, Some(v))], undo: false });
            if queued {
                state.review_cursor = if i + 1 < n { i } else { i.saturating_sub(1) };
                state.preview = next;
            } else if state.tab == Tab::Review {
                update(state, Msg::PreviewStep(1));
            }
            fx
        }
        Msg::JobInfo(run) => {
            state.job_info = run.filter(|r| state.index.run(r).is_some());
            state.job_log.clear();
            match (&state.job_info, state.job_info.as_ref().and_then(|r| state.index.run(r)), &state.library) {
                (Some(id), Some((p, _)), Some(root)) => {
                    vec![Effect::ReadJobLog(id.clone(), increart_core::engine::run_log(&root.join(&p.dir), id))]
                }
                _ => vec![],
            }
        }
        Msg::JobLog(run, lines) => {
            if state.job_info.as_ref() == Some(&run) {
                // lines logged while it was read stay
                let live = std::mem::replace(&mut state.job_log, lines);
                for line in live {
                    if !state.job_log.contains(&line) {
                        state.job_log.push(line);
                    }
                }
            }
            vec![]
        }
        // ------------------------------------------------ review / output library
        Msg::NeedRecipe(id) => {
            if state.recipes.contains_key(&id) {
                return vec![];
            }
            state.recipes.insert(id.clone(), None);
            vec![Effect::LoadRecipe(id)]
        }
        Msg::RecipeLoaded(id, r) => {
            state.recipes.insert(id, r);
            vec![]
        }
        Msg::NeedInputMeta(ids) => {
            let ids: Vec<Id> = ids.into_iter().filter(|id| !state.input_meta.contains_key(id)).collect();
            if ids.is_empty() {
                return vec![];
            }
            for id in &ids {
                state.input_meta.insert(id.clone(), None);
            }
            vec![Effect::LoadInputMeta(ids)]
        }
        Msg::InputMetaLoaded(list) => {
            for (id, m) in list {
                state.input_meta.insert(id, Some(m));
            }
            vec![]
        }
        Msg::GoToOutput(image) => {
            let Some(link) = state.output_link(&image).cloned() else { return vec![] };
            go_to_output(state, link)
        }
        Msg::Checkerboard(on) => {
            state.checkerboard = on;
            vec![]
        }
        Msg::ReviewNewest => {
            state.review_cursor = state.review_queue().len().saturating_sub(1);
            follow_shown(state);
            vec![]
        }
        Msg::ReviewFollow(on) => {
            state.review_follow = on;
            if on {
                if state.config.review_follow.start_newest {
                    state.review_cursor = state.review_queue().len().saturating_sub(1);
                }
                let newest = state.review_queue().iter().map(|o| o.image.id.clone()).max();
                state.follow = Follow { newest, ..Follow::default() };
                follow_shown(state);
            }
            vec![]
        }
        Msg::FollowPace(p) => {
            state.config.review_follow = p;
            save_config(state)
        }
        Msg::FollowPause(on) => {
            state.follow.paused = on;
            vec![]
        }
        Msg::FollowHold(on) => {
            state.follow.held = on;
            vec![]
        }
        Msg::OutputFilter(f) => {
            state.output_filter = f;
            vec![]
        }
        Msg::OutputSearchProjects(on) => {
            if on != state.output_search_projects {
                state.output_search_projects = on;
                if on {
                    state.output_project_search = std::mem::take(&mut state.output_filter.text);
                } else {
                    state.output_filter.text = std::mem::take(&mut state.output_project_search);
                }
            }
            vec![]
        }
        Msg::OutputProjectSearch(q) => {
            state.output_project_search = q;
            vec![]
        }
        Msg::OutputPreset(p) => {
            state.output_preset = p;
            state.output_filter = p.filter();
            vec![]
        }
        Msg::ShowOutputs(source) => {
            let mut fx = update(state, Msg::Tab(Tab::OutputLibrary));
            fx.extend(update(state, Msg::OutputTab(OutputTab::Grid)));
            fx.extend(update(state, Msg::OutputSource(source)));
            // its kept images: the Kept tab, the other filters kept (the notice offers a reset)
            state.output_preset = OutputPreset::Kept;
            state.output_filter.ratings = Ratings::KEPT;
            state.output_notice = true;
            // the panel shows it in "by project"; a project-name search that would hide it goes
            if !state.project_search_shows(&state.output_source) {
                state.output_project_search.clear();
            }
            state.output_reveal = true;
            state.output_reveals += 1;
            fx
        }
        Msg::OutputRevealed => {
            state.output_reveal = false;
            vec![]
        }
        Msg::ResetOutputFilters => {
            state.output_filter = state.output_preset.filter();
            vec![]
        }
        Msg::DismissOutputNotice => {
            state.output_notice = false;
            vec![]
        }
        Msg::OutputSelect(img) => {
            let fx = img.as_ref().map(|i| update(state, Msg::NeedRecipe(i.id.clone()))).unwrap_or_default();
            state.output_selected = img;
            fx
        }
        Msg::AddToCollection(folder, images) => lib(state, LibOp::AddToCollection(folder, images)),
        Msg::NewCollection(parent, images) => lib(state, LibOp::NewCollection(parent, images)),
        Msg::SetCover(p, img) => lib(state, LibOp::SetCover(p, img)),
        Msg::TitleDialog(Some(image)) => {
            let text = state.image_title(&image).unwrap_or_default().to_string();
            state.title_edit = Some(TitleEdit { image, text });
            vec![]
        }
        Msg::TitleDialog(None) => {
            state.title_edit = None;
            vec![]
        }
        Msg::EditTitle(t) => {
            if let Some(e) = &mut state.title_edit {
                e.text = t;
            }
            vec![]
        }
        Msg::CommitTitle => {
            let Some(e) = state.title_edit.take() else { return vec![] };
            let project =
                state.output_link(&e.image).and_then(|o| state.project_of_link(o)).map(|p| p.project.id.clone());
            match project {
                Some(p) => lib(state, LibOp::SetTitle(p, e.image.id, e.text)),
                None => {
                    state.messages_error("That image isn't in a project any more");
                    vec![]
                }
            }
        }
        Msg::PickImage(image, how) => {
            pick_image(state, image, how);
            vec![]
        }
        Msg::PickSet(images) => {
            state.picked = images;
            vec![]
        }
        Msg::RatePicked(v) => {
            let links: Vec<PathBuf> =
                state.picked.iter().filter_map(|i| state.output_link(i)).map(|o| o.path.clone()).collect();
            // rated images leave Review's queue (the sheet keeps them)
            if state.tab == Tab::Review && state.review_view != ReviewView::Sheet {
                state.picked.clear();
            }
            if links.is_empty() {
                return vec![];
            }
            let links = links.into_iter().map(|l| (l, Some(v))).collect();
            lib(state, LibOp::ReviewOutputs { links, undo: false })
        }
        Msg::OpenTile => {
            if state.picked.is_empty() {
                return vec![];
            }
            let picked = state.picked.clone();
            let cells = state.sheet_cells();
            let on_sheet = state.tab == Tab::Review && state.review_view == ReviewView::Sheet;
            let layout = if on_sheet && picked.iter().all(|i| cells.contains_key(i)) {
                crate::tile::TileLayout::at(picked.into_iter().map(|i| (i.clone(), cells[&i].0, cells[&i].1)).collect())
            } else {
                crate::tile::TileLayout::square(picked)
            };
            open_tile(state, layout, None);
            vec![]
        }
        Msg::TileIdeas(ids) => {
            let images: Vec<ImageRef> = ids
                .iter()
                .filter_map(|id| state.index.idea(id))
                .flat_map(|e| e.idea.images.iter().cloned().map(ImageRef::input))
                .collect();
            if images.is_empty() {
                state.messages_info("Nothing to tile: those ideas have no images");
                return vec![];
            }
            open_tile(state, crate::tile::TileLayout::square(images), None);
            vec![]
        }
        Msg::InboxPick(ids) => {
            state.inbox_picked = ids;
            vec![]
        }
        Msg::ReviewPickedIdeas(v) => {
            let ids = std::mem::take(&mut state.inbox_picked);
            ids.into_iter().flat_map(|id| lib(state, LibOp::ReviewIdea(id, v))).collect()
        }
        Msg::LibSelectSet(ids) => {
            state.lib_selected = ids.into_iter().collect();
            vec![]
        }
        Msg::TileGenerateImages => {
            // all of its images (parked ones too), in the strip's order
            let f = &state.generate;
            let images: Vec<ImageRef> = f.images.iter().map(|i| i.image.clone()).collect();
            if images.is_empty() {
                return vec![];
            }
            let start = f.init();
            open_tile(state, crate::tile::TileLayout::square(images), start);
            vec![]
        }
        Msg::TileStart(contents) => {
            if contents.is_empty() {
                state.messages_info("Nothing to tile");
                return vec![];
            }
            if state.tile.is_some() {
                return update(state, Msg::TileAdd(contents, None));
            }
            open_tile(state, crate::tile::TileLayout::square_of(contents), None);
            vec![]
        }
        Msg::CloseTile => {
            state.tile = None;
            vec![]
        }
        Msg::OpenMarkup(image, index) => {
            let slot = index.and_then(|i| Some((state.project.clone()?, i)));
            // an edited copy reopens on its original, as it was drawn
            let (image, edits, replace) = match state.edited_source(&image) {
                Some((orig, edits)) => (orig.clone(), edits.clone(), slot.is_some()),
                None => (image, ImageEdits::default(), false),
            };
            state.markup = Some(Markup { image, edits, undo: vec![], redo: vec![], slot, replace, saving: false });
            vec![]
        }
        Msg::MarkupEdit(edits) => {
            if let Some(m) = &mut state.markup
                && m.edits != edits
            {
                m.undo.push(std::mem::replace(&mut m.edits, edits));
                m.redo.clear();
            }
            vec![]
        }
        Msg::MarkupUndo => {
            if let Some(m) = &mut state.markup
                && let Some(e) = m.undo.pop()
            {
                m.redo.push(std::mem::replace(&mut m.edits, e));
            }
            vec![]
        }
        Msg::MarkupRedo => {
            if let Some(m) = &mut state.markup
                && let Some(e) = m.redo.pop()
            {
                m.undo.push(std::mem::replace(&mut m.edits, e));
            }
            vec![]
        }
        Msg::MarkTools(t) => {
            state.mark_tools = t;
            vec![]
        }
        Msg::MarkupSave(use_it) => {
            let Some(m) = &state.markup else { return vec![] };
            if m.saving {
                return vec![];
            }
            if m.edits.is_empty() {
                state.messages_info("Nothing to save: no crop and no marks");
                return vec![];
            }
            let to = match (&m.slot, use_it) {
                (Some((p, i)), _) => EditedTo::Generate(p.clone(), *i, m.replace),
                (None, true) => EditedTo::UseInGenerate,
                (None, false) => EditedTo::Library,
            };
            let (image, edits) = (m.image.clone(), m.edits.clone());
            let title = edited_title(state, &image);
            if let Some(m) = &mut state.markup {
                m.saving = true;
            }
            lib(state, LibOp::EditedCopy { image, edits, title, to })
        }
        Msg::CloseMarkup => {
            state.markup = None;
            vec![]
        }
        Msg::TileSelect(i) => {
            if let Some(t) = &mut state.tile {
                t.selected = i.filter(|&i| i < t.layout.tiles.len());
            }
            vec![]
        }
        Msg::TileMove(i, c, r) => {
            if let Some(t) = &mut state.tile {
                t.layout.move_tile(i, c, r);
                t.selected = Some(i);
            }
            vec![]
        }
        Msg::TileSpan(i, w, h) => {
            if let Some(t) = &mut state.tile {
                t.layout.set_span(i, w, h);
            }
            vec![]
        }
        Msg::TileCols(n) => {
            if let Some(t) = &mut state.tile {
                t.layout.set_cols(n);
            }
            vec![]
        }
        Msg::TileReflow => {
            if let Some(t) = &mut state.tile {
                t.layout.reflow();
            }
            vec![]
        }
        Msg::TileRemove(i) => {
            if let Some(t) = &mut state.tile {
                t.layout.remove(i);
                t.selected = None;
                if t.layout.tiles.is_empty() {
                    state.tile = None;
                }
            }
            vec![]
        }
        Msg::TileCaption(i, text) => {
            if let Some(tile) = state.tile.as_mut().and_then(|t| t.layout.tiles.get_mut(i)) {
                tile.caption = text;
            }
            vec![]
        }
        Msg::TileAddText(c, r) => {
            if let Some(t) = &mut state.tile {
                t.selected = Some(t.layout.add_text(c, r, "Text".into()));
            }
            vec![]
        }
        Msg::TileText(i, text) => {
            if let Some(Tile { content: TileContent::Text { text: t, .. }, .. }) =
                state.tile.as_mut().and_then(|t| t.layout.tiles.get_mut(i))
            {
                *t = text;
            }
            vec![]
        }
        Msg::TileTextSize(i, px) => {
            if let Some(Tile { content: TileContent::Text { size, .. }, .. }) =
                state.tile.as_mut().and_then(|t| t.layout.tiles.get_mut(i))
            {
                *size = px;
            }
            vec![]
        }
        Msg::TileTitle(title, subtitle) => {
            if let Some(t) = &mut state.tile {
                (t.layout.title, t.layout.subtitle) = (title, subtitle);
            }
            vec![]
        }
        Msg::TileAdd(contents, at) => {
            if let Some(t) = &mut state.tile
                && !contents.is_empty()
            {
                let added = t.layout.add(contents, at);
                t.selected = added.last().copied();
            }
            vec![]
        }
        Msg::TileAddInputs(i) => {
            let Some(inputs) = state
                .tile
                .as_ref()
                .and_then(|t| t.layout.tiles.get(i)?.image().cloned())
                .map(|img| state.image_inputs(&img))
            else {
                return vec![];
            };
            if inputs.is_empty() {
                state.messages_info("This image was made from text only");
            } else if let Some(t) = &mut state.tile {
                t.layout.insert_before(i, inputs.into_iter().map(TileContent::Image).collect());
                t.selected = None;
            }
            vec![]
        }
        Msg::TileInputs(column) => {
            let Some(mut layout) = state.tile.as_ref().map(|t| t.layout.clone()) else { return vec![] };
            let inputs = |i: &ImageRef| state.image_inputs(i);
            let any = if column { layout.inputs_column(inputs) } else { layout.inputs_before(inputs) };
            if !any {
                state.messages_info("These images were made from text only");
            } else if let Some(t) = &mut state.tile {
                t.layout = layout;
                t.selected = None;
            }
            vec![]
        }
        Msg::TileCaptions(fill) => {
            let Some(t) = &state.tile else { return vec![] };
            // only generated images have a seed, run and settings to fill
            // in: pasted ones, inputs and marked-up copies keep their own
            // (hand-written) captions, unless clearing; text blocks keep none
            let captions: Vec<String> = t
                .layout
                .tiles
                .iter()
                .map(|x| match &x.content {
                    TileContent::Image(i) if state.output_link(i).is_some() => state.caption(i, fill),
                    TileContent::Image(_) | TileContent::Pasted(_) if fill != CaptionFill::Clear => x.caption.clone(),
                    _ => String::new(),
                })
                .collect();
            if let Some(t) = &mut state.tile {
                for (x, c) in t.layout.tiles.iter_mut().zip(captions) {
                    x.caption = c;
                }
            }
            vec![]
        }
        Msg::TileCaptionFormat(template) => {
            // generated images only: the others have no seed or run for
            // its placeholders, and keep their hand-written captions
            let Some(t) = &state.tile else { return vec![] };
            let runs: Vec<bool> =
                t.layout.tiles.iter().map(|x| x.image().is_some_and(|i| state.output_link(i).is_some())).collect();
            if let Some(t) = &mut state.tile {
                for (x, _) in t.layout.tiles.iter_mut().zip(runs).filter(|(_, run)| *run) {
                    x.caption = template.clone();
                }
            }
            vec![]
        }
        Msg::TileSettings(s) => {
            state.config.tile = s;
            save_config(state)
        }
        Msg::TileOut(to) => {
            let (Some(t), Some(root)) = (&state.tile, state.library.clone()) else { return vec![] };
            // the captions' placeholders filled
            let mut layout = t.layout.clone();
            for (i, x) in layout.tiles.iter_mut().enumerate() {
                x.caption = state.caption_text(&x.caption, x.image(), i + 1);
            }
            if let Some(t) = &mut state.tile {
                t.working += 1;
            }
            state.busy += 1;
            vec![Effect::ComposeTiles { root, layout, settings: state.config.tile.clone(), to }]
        }
        Msg::TilesDone(to, r) => {
            state.busy = state.busy.saturating_sub(1);
            if let Some(t) = &mut state.tile {
                t.working = t.working.saturating_sub(1);
            }
            let image = match r {
                Ok(i) => i,
                Err(e) => {
                    state.messages_error(format!("Tile: {e}"));
                    return vec![];
                }
            };
            match to {
                TileTarget::Clipboard => {
                    state.clipboard_image = Some(image);
                    state.messages_info("Tiled image copied to the clipboard");
                    vec![]
                }
                TileTarget::File(path) => {
                    state.messages_info(format!("Tiled image saved to {}", path.display()));
                    vec![]
                }
                TileTarget::Library => {
                    let sources = state.tile.iter().flat_map(|t| t.layout.images()).cloned().collect();
                    lib(state, LibOp::IdeaFromTiles { image, sources })
                }
            }
        }
        Msg::OpenExport(images, what) => {
            let dest = state.export.as_ref().map(|e| e.dest.clone()).unwrap_or_default();
            state.export = Some(ExportDialog { images, what, dest, keep_metadata: true, by_id: false });
            vec![]
        }
        Msg::EditExport(e) => {
            state.export = Some(e);
            vec![]
        }
        Msg::RunExport => {
            let Some(e) = state.export.clone() else { return vec![] };
            if e.dest.trim().is_empty() {
                state.messages_error("Choose a destination folder");
                return vec![];
            }
            state.export = None;
            state.busy += 1;
            vec![Effect::Export {
                images: e.images,
                dest: e.dest.trim().into(),
                keep_metadata: e.keep_metadata,
                by_id: e.by_id,
            }]
        }
        Msg::CloseExport => {
            state.export = None;
            vec![]
        }
        Msg::Exported(r) => {
            state.busy = state.busy.saturating_sub(1);
            match r {
                Ok(n) => state.messages_info(format!("Exported {n} image(s)")),
                Err(e) => state.messages_error(format!("Export: {e}")),
            }
            vec![]
        }
        Msg::ShowInFileManager(path) => {
            let abs = match &state.library {
                Some(root) if path.is_relative() => root.join(&path),
                _ => path,
            };
            vec![Effect::ShowInFileManager { path: abs, command: state.config.file_manager.clone() }]
        }
        Msg::OpenLogs(file) => {
            state.logs = Some(LogsView::new(file));
            vec![]
        }
        Msg::Logs(view) => {
            state.logs = view;
            vec![]
        }
        Msg::SaveBugReport => match state.saving_bug_report {
            true => vec![],
            false => update(state, Msg::Pick(PickKind::BugReport)),
        },
        Msg::BugReportSaved(r) => {
            state.saving_bug_report = false;
            match r {
                Ok((path, size)) => state.messages_info(format!(
                    "Saved for a bug report: {} ({:.1} MB). Attach it to the issue; it holds your config, the logs \
                     and the model directories' file names, no images",
                    path.display(),
                    size as f64 / 1e6
                )),
                Err(e) => state.messages_error(format!("Save for a bug report: {e}")),
            }
            vec![]
        }
        Msg::OpenInEditor(path) => vec![Effect::OpenInEditor { path, command: state.config.editor.clone() }],
        Msg::Tick(t) => {
            state.now = t;
            let mut fx = vec![];
            if state.generate_dirty.is_some_and(|since| t - since >= AUTOSAVE_DELAY) {
                fx.extend(flush_generate(state));
            }
            if state.idea_dirty.is_some_and(|since| t - since >= AUTOSAVE_DELAY) {
                fx.extend(flush_idea(state));
            }
            if state.idea_edit_dirty.is_some_and(|since| t - since >= AUTOSAVE_DELAY) {
                fx.extend(flush_idea_edit(state));
            }
            if t - state.alerts.checked >= 1.0 || t < state.alerts.checked {
                state.alerts.checked = t;
                fx.extend(check_alerts(state));
            }
            fx
        }
        Msg::FlushDrafts => {
            let mut fx = flush_generate(state);
            fx.extend(flush_idea(state));
            fx.extend(flush_idea_edit(state));
            fx
        }
        Msg::DraftSaved(Ok(())) => {
            state.saving = state.saving.saturating_sub(1);
            state.draft_saved = Some(state.now);
            vec![]
        }
        Msg::DraftSaved(Err(e)) => {
            state.saving = state.saving.saturating_sub(1);
            state.messages_error(format!("saving a draft: {e}"));
            vec![]
        }
        Msg::QueueGenerate => match model_warning(state) {
            Some(text) => {
                let mut c = Confirm::new(text, "Queue anyway", Msg::QueueModelChecked);
                let sweep =
                    state.generate.sweep.on && state.generate.sweep.vary == increart_core::prompts::SweepVary::Model;
                if !sweep && let Some(j) = state.runnable_alternative(state.generate.preset) {
                    c = c.with_fix(format!("Use {} and queue", state.config.recipes[j].name), Msg::QueueWithPreset(j));
                }
                state.confirm = Some(c);
                vec![]
            }
            None => update(state, Msg::QueueModelChecked),
        },
        Msg::QueueModelChecked => {
            let unnamed = unnamed_refs(state);
            let tags: Vec<String> = unnamed.iter().map(|n| format!("<image{n}>")).collect();
            let (list, them) = match tags.as_slice() {
                [] => return update(state, Msg::QueueChecked),
                [one] => (one.clone(), "it"),
                [rest @ .., last] => (format!("{} and {last}", rest.join(", ")), "them"),
            };
            let text = format!(
                "{list} {} named in the prompt{}. The model gets {them} anyway, \
                 but without a name the prompt can't say what to do with {them}.",
                if unnamed.len() == 1 { "isn't" } else { "aren't" },
                if state.generate.sweep.on { " or any of the sweep's prompts" } else { "" }
            );
            state.confirm = Some(Confirm::new(text, "Queue anyway", Msg::QueueChecked));
            vec![]
        }
        Msg::QueueChecked if state.generate.sweep.on => update(state, Msg::QueueSweep(false)),
        Msg::QueueSweep(confirmed) => queue_sweep(state, confirmed),
        Msg::JobOrder(order) => {
            if state.config.job_order == order {
                return vec![];
            }
            state.config.job_order = order;
            let mut fx = save_config(state);
            fx.push(Effect::SetJobOrder(order));
            fx
        }
        Msg::SweepOrderAnswer(answer) => {
            let Some(big) = state.sweep_order_ask.take() else { return vec![] };
            let Some(spread) = answer else { return vec![] };
            let mut fx = if spread { update(state, Msg::JobOrder(Order::BySweep)) } else { vec![] };
            state.sweep_order_asked = true;
            fx.extend(queue_sweep(state, big));
            state.sweep_order_asked = false;
            fx
        }
        Msg::QueueChecked => {
            let Some(project) = state.project.clone() else {
                state.messages_error("Select a project first");
                return vec![];
            };
            let f = &state.generate;
            if f.prompt.trim().is_empty() {
                state.messages_error("The prompt is empty");
                return vec![];
            }
            // the blanks filled from the blanks table
            let filled = increart_core::prompts::fill_slots(f.prompt.clone(), &f.sweep.slots);
            let unfilled = increart_core::prompts::slot_names([filled.as_str()]);
            if let Some(n) = unfilled.first() {
                let blank = format!("{{{n}}}");
                let at = increart_core::prompts::slots(&f.prompt).into_iter().find(|r| f.prompt[r.clone()] == blank);
                state.messages_error(format!("Fill in {blank} first (in the blanks table, or type over it)"));
                if let Some(r) = at {
                    let chars = |b: usize| state.generate.prompt[..b].chars().count();
                    state.prompt_select = Some(chars(r.start)..chars(r.end));
                }
                return vec![];
            }
            let mut run = f.clone();
            run.prompt = filled;
            let f = &run;
            if !f.width.is_multiple_of(32) || !f.height.is_multiple_of(32) || f.seeds == 0 {
                state.messages_error("Size must be a multiple of 32 and seeds at least 1");
                return vec![];
            }
            let Some(preset) = state.config.recipes.get(f.preset).cloned() else {
                state.messages_error("No model preset");
                return vec![];
            };
            // once per session: the run would only wait in the queue
            if state.servers.is_empty() && state.background.is_none() && !state.no_server_warned {
                state.no_server_warned = true;
                state.no_server_warning = true;
                return vec![];
            }
            if still_saving(state, &project, &f.label) {
                return vec![];
            }
            let queued = Box::new(Queued { project: project.clone(), form: Box::new(f.to_draft(&state.config)) });
            state.queueing.push((project.clone(), f.label.clone()));
            let op = LibOp::CreateRun {
                project,
                form: f.clone(),
                preset,
                model_dirs: state.config.programs.sd_server.model_dirs.clone(),
                queue: state.engine_running,
                queued,
            };
            if !state.engine_running {
                state.messages_info("Run saved; it will be generated once servers are connected");
            }
            lib(state, op)
        }
        Msg::CancelRun(id, how) => {
            // its GPUs going quiet is no news (Finish & stop still says
            // when the last images are done)
            if how == Stop::Now {
                state.alerts.stopped_now.insert(id.clone());
            }
            if how == Stop::Finish {
                state.finishing.insert(id.clone());
            } else {
                state.finishing.remove(&id);
            }
            vec![Effect::CancelRun(id, how)]
        }
        Msg::StopAll(how) => {
            let runs: Vec<Id> = state.active_runs.iter().cloned().collect();
            update(state, Msg::CancelRuns(runs, how))
        }
        Msg::CancelRuns(runs, how) => runs.into_iter().flat_map(|id| update(state, Msg::CancelRun(id, how))).collect(),

        // ------------------------------------------------ review
        Msg::ReviewScopeFolder(f) => {
            leave_sweep(state);
            state.review_scope = None;
            state.review_folder = Some(f);
            state.review_cursor = 0;
            vec![]
        }
        Msg::ReviewScope(s) => {
            leave_sweep(state);
            state.review_folder = None;
            state.review_scope = s;
            state.review_cursor = 0;
            vec![]
        }
        Msg::ReviewSweep(project, name) => {
            // the image being looked at stays selected on the sheet
            let current = state.review_queue().get(state.review_cursor).map(|o| o.path.clone());
            if state.review_sweep.as_ref() != Some(&name) || state.review_scope.as_ref() != Some(&project) {
                state.sheet_filter.clear();
            }
            if state.review_sweep.as_ref() != Some(&name) || state.review_scope.as_ref() != Some(&project) {
                state.picked.clear();
            }
            state.review_folder = None;
            state.review_scope = Some(project);
            state.review_sweep = Some(name);
            state.review_run = None;
            state.review_cursor = 0;
            state.review_pinned = None;
            state.review_view = ReviewView::Sheet;
            state.sheet_cell = state
                .sheet()
                .and_then(|sh| {
                    sh.rows.iter().enumerate().find_map(|(r, row)| {
                        let c = row
                            .cells
                            .iter()
                            .position(|c| matches!(c, SheetCell::Image(o) if Some(&o.path) == current.as_ref()))?;
                        Some((r, c))
                    })
                })
                .unwrap_or((0, 0));
            vec![]
        }
        Msg::ReviewRun(project, run) => {
            leave_sweep(state);
            state.review_folder = None;
            state.review_scope = Some(project);
            state.review_run = Some(run);
            state.review_cursor = 0;
            vec![]
        }
        Msg::ReviewOrder(o) => {
            state.config.review_order = o;
            save_config(state)
        }
        Msg::ReviewHideReviewed(on) => {
            state.config.review_show_reviewed = !on;
            save_config(state)
        }
        Msg::ReviewAllRuns(on) => {
            state.review_all_runs = on;
            vec![]
        }
        Msg::ReviewSearch(q) => {
            state.review_search = q;
            vec![]
        }
        Msg::ProjectRuns(on) => {
            state.config.hide_project_runs = !on;
            save_config(state)
        }
        Msg::ProjectOrder(o) => {
            state.config.project_order = o;
            save_config(state)
        }
        Msg::FolderOrder(o) => {
            state.config.folder_order = increart_core::config::FolderOrder(o);
            save_config(state)
        }
        Msg::NarrowToSiblings(on) => {
            state.config.project_click_keeps_scope = !on;
            save_config(state)
        }
        Msg::ShowInfo(panel, on) => {
            if on {
                state.info_hidden.remove(&panel);
            } else {
                state.info_hidden.insert(panel);
            }
            vec![]
        }
        Msg::ToggleProjectFolder(f) => {
            let collapse = !state.folder_collapsed(&f);
            set_collapsed(state, |c| {
                if collapse {
                    c.push(f);
                } else {
                    c.retain(|g| g != &f);
                }
            })
        }
        Msg::CollapseProjectFolders(collapse) => {
            let all = state.index.project_folders.clone();
            set_collapsed(state, |c| *c = if collapse { all } else { vec![] })
        }
        Msg::UiZoom { percent, save } => {
            let p = percent.clamp(UI_ZOOM_MIN, UI_ZOOM_MAX);
            state.config.ui_zoom = if p == 100 { 0 } else { p };
            if save { save_config(state) } else { vec![] }
        }
        Msg::UiZoomStep(up) => {
            let z = state.ui_zoom();
            let percent =
                if up { (z / UI_ZOOM_STEP + 1) * UI_ZOOM_STEP } else { (z - 1) / UI_ZOOM_STEP * UI_ZOOM_STEP };
            update(state, Msg::UiZoom { percent, save: true })
        }
        Msg::SheetZoom { size, save } => {
            state.config.sheet_cell = size.clamp(SHEET_CELL_MIN, SHEET_CELL_MAX);
            if save { save_config(state) } else { vec![] }
        }
        Msg::GridZoom { grid, size, save } => {
            let size = size.clamp(GRID_CELL_MIN, GRID_CELL_MAX);
            match grid {
                ZoomGrid::Review => state.config.review_grid_cell = size,
                ZoomGrid::Output => state.config.output_grid_cell = size,
                ZoomGrid::Library => state.config.library_grid_cell = size,
                ZoomGrid::Inbox => state.config.inbox_grid_cell = size,
            }
            if save { save_config(state) } else { vec![] }
        }
        Msg::SheetFilter(f) => {
            state.sheet_filter = f;
            state.sheet_cell.0 = 0;
            vec![]
        }
        Msg::SheetCell(r, c) => {
            state.sheet_cell = (r, c);
            vec![]
        }
        Msg::SheetRows(rows) => {
            state.config.sheet_rows = rows;
            save_config(state)
        }
        Msg::SheetBands(k) => {
            state.sheet_bands = k.max(1);
            vec![]
        }
        Msg::SheetMove(dl, dc) => {
            if let Some(sh) = state.sheet() {
                let (rows, cols) = (sh.rows.len() as isize, sh.seeds.len() as isize);
                if rows == 0 || cols == 0 {
                    return vec![];
                }
                let k = state.sheet_bands.max(1) as isize;
                let (r, c) = (state.sheet_cell.0 as isize, state.sheet_cell.1 as isize);
                let line = (r / k + dl).clamp(0, (rows - 1) / k);
                // the last line may have fewer bands
                let bands = (rows - line * k).min(k);
                let x = ((r % k).min(bands - 1) * cols + c + dc).clamp(0, bands * cols - 1);
                state.sheet_cell = ((line * k + x / cols) as usize, (x % cols) as usize);
            }
            vec![]
        }
        Msg::SheetRate(v) => {
            let (r, c) = state.sheet_cell;
            let link = state.sheet().and_then(|sh| match sh.rows.get(r)?.cells.get(c)? {
                SheetCell::Image(o) => Some(o.path.clone()),
                _ => None,
            });
            let Some(link) = link else { return vec![] };
            // on to the next cell, as single review goes on: along the row,
            // then the next row's first
            let fx = lib(state, LibOp::ReviewOutputs { links: vec![(link, Some(v))], undo: false });
            if let Some(sh) = state.sheet() {
                state.sheet_cell = if c + 1 < sh.seeds.len() {
                    (r, c + 1)
                } else if r + 1 < sh.rows.len() {
                    (r + 1, 0)
                } else {
                    (r, c)
                };
            }
            fx
        }
        Msg::SheetRow(r, v) => {
            let links: Vec<PathBuf> = state
                .sheet()
                .and_then(|sh| {
                    let row = sh.rows.get(r)?;
                    Some(
                        row.cells
                            .iter()
                            .filter_map(|c| match c {
                                SheetCell::Image(o) if o.verdict.is_none() => Some(o.path.clone()),
                                _ => None,
                            })
                            .collect(),
                    )
                })
                .unwrap_or_default();
            if links.is_empty() {
                return vec![];
            }
            let links = links.into_iter().map(|l| (l, Some(v))).collect();
            lib(state, LibOp::ReviewOutputs { links, undo: false })
        }
        Msg::SheetNewSweep => {
            let (Some(project), Some(sweep)) = (state.review_scope.clone(), state.review_sweep.clone()) else {
                return vec![];
            };
            // the picked images' runs, in the sheet's order
            let picked: Vec<&Id> =
                state.picked.iter().filter_map(|i| state.output_link(i)).map(|o| &o.name.run).collect();
            let mut runs: Vec<Id> = state
                .index
                .project(&project)
                .map(|p| {
                    p.runs
                        .iter()
                        .filter(|r| r.sweep.as_ref().is_some_and(|s| s.name == sweep) && picked.contains(&&r.id))
                        .map(|r| r.id.clone())
                        .collect()
                })
                .unwrap_or_default();
            runs.sort();
            if runs.is_empty() {
                return vec![];
            }
            state.new_sweep = Some(NewSweepAsk { project: project.clone(), sweep, runs });
            vec![Effect::LoadHistory(HistoryKey::Project(project))]
        }
        Msg::SheetMoreSeeds(n) => {
            let runs: Vec<Id> = state
                .sheet()
                .map(|sh| {
                    sh.rows
                        .iter()
                        .filter(|row| {
                            row.cells
                                .iter()
                                .any(|c| matches!(c, SheetCell::Image(o) if o.verdict == Some(Verdict::Good)))
                        })
                        .map(|row| row.run.id.clone())
                        .collect()
                })
                .unwrap_or_default();
            if runs.is_empty() {
                state.messages_error("No style has a Good image yet");
                return vec![];
            }
            runs.into_iter().flat_map(|r| update(state, Msg::MoreSeeds(r, n))).collect()
        }
        Msg::RecipeInGenerate(image) => {
            let Some(name) = state.output_link(&image).map(|o| o.name.clone()) else {
                return vec![];
            };
            let fx = update(state, Msg::RunInGenerate(name.run));
            let f = &mut state.generate;
            f.seeds = 1;
            f.first_seed = name.seed;
            f.sweep.on = false;
            fx
        }
        Msg::RunInGenerate(id) => {
            let Some((p, run)) = state.index.run(&id).map(|(p, r)| (p.project.id.clone(), r.clone())) else {
                return vec![];
            };
            let mut fx = update(state, Msg::OpenInGenerate(p.clone()));
            let f = &state.generate;
            if !f.prompt.trim().is_empty() {
                let (t, x) = (f.prompt.clone(), f.negative.clone());
                fx.extend(record(state, HistoryKey::Project(p), HistoryKind::BeforeRestore, t, x));
            }
            let mut f = GenerateForm::from_run(&run, &state.config);
            f.label = next_label(state);
            f.first_seed = state.next_seed();
            f.sweep.suggestions = std::mem::take(&mut state.generate.sweep.suggestions);
            f.sweep.batch_ask = state.generate.sweep.batch_ask.take();
            f.sweep.enhance_ask = state.generate.sweep.enhance_ask.take();
            state.generate = f;
            runnable_preset(state);
            mark_generate(state);
            fx
        }
        Msg::ReviewCursor(i) => {
            state.review_cursor = i.min(state.review_queue().len().saturating_sub(1));
            unfollow(state);
            vec![]
        }
        Msg::ReviewNext(d) => {
            let n = state.review_queue().len();
            if n > 0 {
                state.review_cursor = (state.review_cursor as isize + d).clamp(0, n as isize - 1) as usize;
            }
            // on to the next keeps following, from the start of its time
            if d > 0 {
                follow_shown(state);
            } else {
                unfollow(state);
            }
            vec![]
        }
        Msg::ReviewCurrent(v) => {
            let queue = state.review_queue();
            // just moved on by Follow: meant for the image that was there
            let just_moved = state
                .follow
                .moved
                .as_ref()
                .filter(|(at, _)| state.review_follow && state.now - at < Follow::GRACE)
                .and_then(|(_, img)| queue.iter().find(|o| &o.image == img));
            let Some(link) = just_moved.or(queue.get(state.review_cursor)).map(|o| o.path.clone()) else {
                return vec![];
            };
            lib(state, LibOp::ReviewOutputs { links: vec![(link, Some(v))], undo: false })
        }
        Msg::ReviewView(v) => {
            if v == ReviewView::Compare {
                return update(state, Msg::Compare);
            }
            if v == ReviewView::Sheet && state.review_sweep.is_none() {
                return vec![];
            }
            state.review_view = v;
            state.review_pinned = None;
            vec![]
        }
        Msg::ToggleReviewGrid => {
            let v = if state.review_view == ReviewView::Grid { ReviewView::Single } else { ReviewView::Grid };
            update(state, Msg::ReviewView(v))
        }
        Msg::Compare => {
            if state.review_view == ReviewView::Compare {
                state.review_view = ReviewView::Single;
                state.review_pinned = None;
                return vec![];
            }
            let queue = state.review_queue();
            let Some(cur) = queue.get(state.review_cursor).map(|o| o.image.clone()) else { return vec![] };
            let has_next = state.review_cursor + 1 < queue.len();
            state.review_pinned = Some(cur);
            state.review_view = ReviewView::Compare;
            if has_next {
                state.review_cursor += 1;
            }
            vec![]
        }
        // the sides change places: the pinned image on the other side
        Msg::SwapCompare => {
            state.compare_flipped = !state.compare_flipped;
            vec![]
        }
        Msg::ReviewPinned(v) => {
            let link = state.review_pinned.as_ref().and_then(|a| state.output_link(a)).map(|o| o.path.clone());
            match link {
                Some(link) => lib(state, LibOp::ReviewOutputs { links: vec![(link, Some(v))], undo: false }),
                None => vec![],
            }
        }
        Msg::OutputTab(t) => {
            state.output_tab = t;
            state.output_compare = None;
            fv_sync(state);
            relist_if_stale(state)
        }
        Msg::OutputSource(s) => {
            // on the Folder tab, the folder view goes to the source too
            let path = source_path(state, &s);
            if s != state.output_source {
                state.picked.clear();
            }
            state.output_source = s;
            state.output_notice = false;
            if state.tab == Tab::OutputLibrary && state.output_tab == OutputTab::Folder {
                return fv_go(state, path, true);
            }
            vec![]
        }
        Msg::DeleteOutput(link) => lib(state, LibOp::DeleteOutputs(vec![link])),
        Msg::RestoreOutput(link) => lib(state, LibOp::RestoreOutputs(vec![link])),
        Msg::DeleteDiscardedOutput(link) => lib(state, LibOp::DeleteDiscardedOutputs(vec![link])),
        Msg::EmptyDiscardedOutputs => {
            let links: Vec<_> = state.discarded_outputs().iter().map(|o| o.path.clone()).collect();
            if links.is_empty() {
                return vec![];
            }
            lib(state, LibOp::DeleteDiscardedOutputs(links))
        }
        Msg::DeleteShownRejected => {
            let links: Vec<_> = state
                .output_source_images()
                .iter()
                .filter(|o| o.verdict == Some(Verdict::Bad))
                .map(|o| o.path.clone())
                .collect();
            if links.is_empty() {
                return vec![];
            }
            lib(state, LibOp::DeleteOutputs(links))
        }
        Msg::Ask(c) => {
            state.confirm = Some(c);
            vec![]
        }
        Msg::ConfirmYes => match state.confirm.take() {
            Some(c) => update(state, *c.then),
            None => vec![],
        },
        Msg::ConfirmNo => {
            state.confirm = None;
            vec![]
        }
        Msg::ConfirmFix => match state.confirm.take().and_then(|c| c.fix) {
            Some((_, then)) => update(state, *then),
            None => vec![],
        },
        Msg::QueueWithPreset(i) => {
            if i < state.config.recipes.len() {
                state.generate.preset = i;
                mark_generate(state);
            }
            update(state, Msg::QueueGenerate)
        }
        Msg::ReviewLink(link, verdict) => {
            lib(state, LibOp::ReviewOutputs { links: vec![(link, verdict)], undo: false })
        }
        // the last rating, of one image or of all the picked at once
        Msg::UndoReview => match state.review_undo.pop() {
            Some(links) => lib(state, LibOp::ReviewOutputs { links, undo: true }),
            None => vec![],
        },

        // ------------------------------------------------ runtime reports
        Msg::Scanned(ix) => {
            set_index(state, ix);
            relist(state)
        }
        Msg::OpDone(result, ix) => {
            state.busy = state.busy.saturating_sub(1);
            if let Some(ix) = ix {
                set_index(state, ix);
            }
            let mut fx = relist(state);
            match result {
                Err(e) => {
                    if let Some(m) = &mut state.markup {
                        m.saving = false;
                    }
                    // (which op failed isn't known: a failed run or sweep
                    // can be queued again)
                    state.queueing.clear();
                    state.messages_error(e)
                }
                Ok(ok) => fx.extend(op_done(state, ok)),
            }
            fx
        }
        Msg::Engine(e) => engine_event(state, e),
        Msg::RunsQueued(runs) => {
            state.active_runs = runs.into_iter().collect();
            let active = &state.active_runs;
            state.finishing.retain(|r| active.contains(r));
            state.progress.clear();
            state.sampling_times.clear();
            vec![]
        }
        Msg::Pick(kind) => {
            let start = match kind {
                PickKind::LibraryFolder => match &state.phase {
                    Phase::Library(d) if !d.path.is_empty() => {
                        let p = PathBuf::from(&d.path);
                        Some(if p.is_dir() { p } else { p.parent().map(PathBuf::from).unwrap_or(p) })
                    }
                    _ => None,
                },
                PickKind::ModelDir(i) => {
                    let dirs = &state.config.programs.sd_server.model_dirs;
                    dirs.get(i).or(dirs.last()).filter(|d| !d.trim().is_empty()).map(PathBuf::from)
                }
                // the current file's folder, else the model directory
                PickKind::RecipeFile(slot) => {
                    let dirs = &state.config.programs.sd_server.model_dirs;
                    let on_disk = state.recipe_editor.as_ref().and_then(|e| {
                        let file = e.rows.get(e.selected)?.preset.files.get(*MODEL_SLOTS.get(slot)?)?;
                        increart_core::config::model_file_on_disk(file, dirs)
                    });
                    on_disk.and_then(|p| p.parent().map(PathBuf::from)).or_else(|| dirs.first().map(PathBuf::from))
                }
                // the current LoRA's folder, else the loras folder
                PickKind::RecipeLora(i) => {
                    let dirs = &state.config.programs.sd_server.model_dirs;
                    let on_disk = state.recipe_editor.as_ref().and_then(|e| {
                        let lora = e.rows.get(e.selected)?.preset.loras.get(i)?;
                        increart_core::config::model_file_on_disk(&lora.file, dirs)
                    });
                    on_disk.and_then(|p| p.parent().map(PathBuf::from)).or_else(|| {
                        increart_core::config::lora_dirs(dirs)
                            .into_iter()
                            .next()
                            .or_else(|| dirs.first().map(PathBuf::from))
                    })
                }
                PickKind::SdServerBinary => {
                    PathBuf::from(&state.config.programs.sd_server.binary).parent().map(PathBuf::from)
                }
                PickKind::KoboldBinary => PathBuf::from(kobold_binary(state)).parent().map(PathBuf::from),
                PickKind::ExportDir => state.export.as_ref().map(|e| PathBuf::from(&e.dest)),
                PickKind::Pe(f) => match state.pe.editor.as_ref().map(|e| &e.draft.run) {
                    Some(PeRun::KoboldCpp { binary, model, mmproj, config, .. }) => {
                        let p = match f {
                            PeFile::Binary => binary,
                            PeFile::Model => model,
                            PeFile::Mmproj if mmproj.is_empty() => model,
                            PeFile::Mmproj => mmproj,
                            PeFile::Config if config.is_empty() => binary,
                            PeFile::Config => config,
                        };
                        PathBuf::from(p).parent().map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())
                    }
                    _ => None,
                },
                PickKind::Kobold(i, f) => {
                    let entry = match &state.phase {
                        Phase::Servers(d) => d.draft.servers.get(i),
                        _ => None,
                    };
                    let p = match entry {
                        Some(ServerConfig::Kobold(k)) => match (f, &k.run, &k.chat) {
                            (PeFile::Binary, increart_core::kobold::KoboldRun::Start { binary, .. }, _) => {
                                binary.clone()
                            }
                            (PeFile::Config, increart_core::kobold::KoboldRun::Start { config, binary, .. }, _) => {
                                if config.is_empty() {
                                    binary.clone()
                                } else {
                                    config.clone()
                                }
                            }
                            (PeFile::Model, _, Some(c)) => c.model.clone(),
                            (PeFile::Mmproj, _, Some(c)) => {
                                if c.mmproj.is_empty() {
                                    c.model.clone()
                                } else {
                                    c.mmproj.clone()
                                }
                            }
                            _ => String::new(),
                        },
                        _ => String::new(),
                    };
                    let models = state.config.programs.sd_server.model_dirs.first().map(PathBuf::from);
                    PathBuf::from(p).parent().map(PathBuf::from).filter(|p| p.is_dir()).or(models)
                }
                PickKind::WizardBinary | PickKind::WizardModels => match &state.phase {
                    Phase::Servers(ServerDialog { kobold: Some(k), .. }) => {
                        let p = PathBuf::from(if kind == PickKind::WizardBinary { &k.binary } else { &k.models_dir });
                        if p.is_dir() { Some(p) } else { p.parent().map(PathBuf::from).filter(|p| p.is_dir()) }
                    }
                    _ => None,
                },
                // the library itself: the picker starts beside it, named after it
                PickKind::LibraryArchiveOut(_) => state.library.clone(),
                PickKind::LibraryArchiveIn => {
                    let lib =
                        state.library.clone().or_else(|| state.config.recent_libraries.first().map(PathBuf::from));
                    lib.and_then(|l| l.parent().map(PathBuf::from))
                }
                PickKind::LibraryUnpackInto => state.unpack_from.as_ref().and_then(|a| a.parent()).map(PathBuf::from),
                PickKind::ImportFiles | PickKind::IdeaImages | PickKind::GenerateRefs | PickKind::TileFile(_) => None,
                // Downloads, where it's found to attach
                PickKind::BugReport => Some(PathBuf::from(increart_core::config::full_path("~/Downloads"))),
            };
            vec![Effect::Pick { kind, start }]
        }
        Msg::Picked(_, paths) if paths.is_empty() => vec![],
        Msg::Picked(kind, mut paths) => match kind {
            PickKind::LibraryFolder => {
                if let Phase::Library(d) = &mut state.phase {
                    d.path = paths.remove(0).to_string_lossy().into_owned();
                    d.error = None;
                }
                vec![]
            }
            PickKind::ImportFiles => update(state, Msg::DropFiles(paths)),
            PickKind::IdeaImages => lib(state, LibOp::ImportImages(paths)),
            PickKind::GenerateRefs => update(state, Msg::DropRefs(paths)),
            PickKind::TileFile(format) => {
                // the name says what's in the file: another image extension
                // is replaced, anything else kept and added to
                let mut path = paths.remove(0);
                let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                let fits = ext == format.ext() || (ext == "jpeg" && format == TileFormat::Jpeg);
                if !fits {
                    let image_ext = ext == "jpeg" || TileFormat::available().iter().any(|f| f.ext() == ext);
                    if image_ext {
                        path.set_extension(format.ext());
                    } else {
                        let name =
                            format!("{}.{}", path.file_name().unwrap_or_default().to_string_lossy(), format.ext());
                        path.set_file_name(name);
                    }
                }
                update(state, Msg::TileOut(TileTarget::File(path)))
            }
            PickKind::LibraryArchiveOut(format) => {
                let Some(root) = state.library.clone() else { return vec![] };
                let mut dest = paths.remove(0);
                if increart_core::archive::ArchiveFormat::of_path(&dest) != Some(format) {
                    let name = format!("{}.{}", dest.file_name().unwrap_or_default().to_string_lossy(), format.ext());
                    dest.set_file_name(name);
                }
                state.archiving = Some(Archiving { archive: dest.clone(), export: true, done: 0, total: 0 });
                vec![Effect::ExportLibrary { root, dest, format }]
            }
            PickKind::LibraryArchiveIn => {
                state.unpack_from = Some(paths.remove(0));
                update(state, Msg::Pick(PickKind::LibraryUnpackInto))
            }
            PickKind::LibraryUnpackInto => {
                let Some(archive) = state.unpack_from.take() else { return vec![] };
                let into = paths.remove(0);
                state.archiving = Some(Archiving { archive: archive.clone(), export: false, done: 0, total: 0 });
                vec![Effect::UnpackArchive { archive, into }]
            }
            PickKind::BugReport => {
                let mut dest = paths.remove(0);
                if dest.extension().is_none_or(|e| !e.eq_ignore_ascii_case("zip")) {
                    dest.set_file_name(format!("{}.zip", dest.file_name().unwrap_or_default().to_string_lossy()));
                }
                state.saving_bug_report = true;
                let config = state.config_path.clone();
                vec![Effect::SaveBugReport {
                    config,
                    model_dirs: state.config.programs.sd_server.model_dirs.clone(),
                    validations: state.validations_path.clone(),
                    dest,
                }]
            }
            PickKind::ExportDir => {
                if let Some(e) = &mut state.export {
                    e.dest = paths.remove(0).to_string_lossy().into_owned();
                }
                vec![]
            }
            PickKind::SdServerBinary => {
                let mut c = state.config.programs.sd_server.clone();
                c.binary = paths.remove(0).to_string_lossy().into_owned();
                update(state, Msg::SetSdServer(c))
            }
            PickKind::KoboldBinary => {
                let mut fx = update(state, Msg::SetKoboldBinary(paths.remove(0).to_string_lossy().into_owned()));
                fx.extend(update(state, Msg::CheckKoboldVersion));
                fx
            }
            PickKind::Pe(f) => {
                let Some(e) = &state.pe.editor else { return vec![] };
                let mut draft = e.draft.clone();
                if let PeRun::KoboldCpp { binary, model, mmproj, config, .. } = &mut draft.run {
                    let path = paths.remove(0).to_string_lossy().into_owned();
                    *match f {
                        PeFile::Binary => binary,
                        PeFile::Model => model,
                        PeFile::Mmproj => mmproj,
                        PeFile::Config => config,
                    } = path;
                }
                update(state, Msg::PeEditorEdit(draft))
            }
            PickKind::Kobold(i, f) => {
                let path = paths.remove(0).to_string_lossy().into_owned();
                let Phase::Servers(d) = &state.phase else { return vec![] };
                let Some(ServerConfig::Kobold(k)) = d.draft.servers.get(i) else { return vec![] };
                let mut k = k.clone();
                match (f, &mut k.run, &mut k.chat) {
                    (PeFile::Binary, increart_core::kobold::KoboldRun::Start { binary, .. }, _) => *binary = path,
                    (PeFile::Config, increart_core::kobold::KoboldRun::Start { config, .. }, _) => *config = path,
                    (PeFile::Model, _, Some(c)) => c.model = path,
                    (PeFile::Mmproj, _, Some(c)) => c.mmproj = path,
                    _ => return vec![],
                }
                update(state, Msg::EditServer(i, ServerConfig::Kobold(k)))
            }
            PickKind::WizardBinary | PickKind::WizardModels => {
                let path = paths.remove(0).to_string_lossy().into_owned();
                let Phase::Servers(ServerDialog { kobold: Some(k), .. }) = &state.phase else { return vec![] };
                let mut k = (**k).clone();
                if kind == PickKind::WizardBinary {
                    k.binary = path;
                } else {
                    k.models_dir = path;
                }
                let mut fx = update(state, Msg::EditKoboldWizard(k));
                fx.extend(update(state, Msg::CheckWizardBinary));
                fx
            }
            PickKind::RecipeFile(slot) => {
                let id =
                    increart_core::config::model_file_id(&paths.remove(0), &state.config.programs.sd_server.model_dirs);
                let Some(e) = &mut state.recipe_editor else { return vec![] };
                let Some(name) = MODEL_SLOTS.get(slot) else { return vec![] };
                match id {
                    Ok(id) => {
                        e.error = None;
                        if let Some(r) = e.rows.get_mut(e.selected) {
                            r.preset.files.insert(name.to_string(), id);
                        }
                    }
                    Err(why) => e.error = Some(why),
                }
                vec![]
            }
            PickKind::RecipeLora(i) => {
                let id =
                    increart_core::config::lora_file_id(&paths.remove(0), &state.config.programs.sd_server.model_dirs);
                let Some(e) = &mut state.recipe_editor else { return vec![] };
                match id {
                    Ok(id) => {
                        e.error = None;
                        if let Some(l) = e.rows.get_mut(e.selected).and_then(|r| r.preset.loras.get_mut(i)) {
                            l.file = id;
                        }
                    }
                    Err(why) => e.error = Some(why),
                }
                vec![]
            }
            PickKind::ModelDir(i) => {
                let mut c = state.config.programs.sd_server.clone();
                let dir = paths.remove(0).to_string_lossy().into_owned();
                if let Some(d) = c.model_dirs.get_mut(i) {
                    *d = dir;
                } else if !c.model_dirs.contains(&dir) {
                    c.model_dirs.push(dir);
                }
                update(state, Msg::SetSdServer(c))
            }
        },
        Msg::DropRefs(paths) => {
            if paths.is_empty() || state.project.is_none() {
                return vec![];
            }
            lib(state, LibOp::ImportRefs(paths))
        }
        Msg::RemoveDraftImage(i) => {
            if i < state.draft.images.len() {
                state.draft.images.remove(i);
                state.idea_dirty = Some(state.now);
            }
            vec![]
        }
        Msg::SetSdServer(c) => {
            if c == state.config.programs.sd_server {
                return vec![];
            }
            state.config.programs.sd_server = c;
            save_config(state)
        }
        Msg::SetKoboldBinary(b) => {
            if b == state.config.programs.koboldcpp {
                return vec![];
            }
            state.config.set_koboldcpp(b);
            save_config(state)
        }
        Msg::SetTimeouts(mut t) => {
            for s in [&mut t.server_start, &mut t.request, &mut t.chat, &mut t.download] {
                *s = (*s).max(1);
            }
            if t == state.config.timeouts {
                return vec![];
            }
            state.config.timeouts = t;
            save_config(state)
        }
        Msg::CheckKoboldVersion => kobold_version_once(state),
        Msg::KoboldVersion(binary, r) => {
            if let Phase::Servers(d) = &mut state.phase {
                d.kobold_version = Some((binary, r));
            }
            vec![]
        }
        // ------------------------------------------------ folder view
        Msg::FvGo(path) => fv_go(state, path, true),
        Msg::FvBack => match state.folder_view.back.pop() {
            Some(p) => {
                let cur = state.folder_view.path.clone();
                state.folder_view.forward.push(cur);
                fv_go(state, p, false)
            }
            None => vec![],
        },
        Msg::FvForward => match state.folder_view.forward.pop() {
            Some(p) => {
                let cur = state.folder_view.path.clone();
                state.folder_view.back.push(cur);
                fv_go(state, p, false)
            }
            None => vec![],
        },
        Msg::FvUp => {
            let fv = &state.folder_view;
            match fv.path.parent().filter(|p| p.starts_with(&fv.root)).map(PathBuf::from) {
                Some(p) => fv_go(state, p, true),
                None => vec![],
            }
        }
        Msg::FvRefresh => {
            let path = state.folder_view.path.clone();
            state.folder_view.listed = Some(path.clone());
            fv_list(state, path)
        }
        Msg::FvListed(path, mut entries) => {
            if path == std::path::Path::new("collections") && !entries.iter().any(|e| e.name == "smart") {
                let n = state.kept_outputs().len() as u64;
                let info = "smart folders (read-only)".to_string();
                entries.push(FvEntry { name: "smart".into(), kind: EntryKind::Smart, size: n, modified: None, info });
            }
            let smart = is_smart(&path);
            let fv = fv_for(state, &path);
            if fv.place == FvPlace::Inbox && path == std::path::Path::new("input") {
                entries.retain(|e| e.name != "library"); // the Input library tab's
            }
            if path == fv.path {
                // smart folders keep their own order (newest day first, …)
                if !smart {
                    entries.sort_by(|a, b| {
                        // smart/, then folders, then projects / runs / ideas, then files
                        let rank = |k: &EntryKind| match k {
                            EntryKind::Smart => 0,
                            EntryKind::Folder => 1,
                            k if k.is_dir() => 2,
                            _ => 3,
                        };
                        let key = |e: &FvEntry| (rank(&e.kind), e.name.to_lowercase());
                        key(a).cmp(&key(b))
                    });
                }
                fv.selected.retain(|n| entries.iter().any(|e| &e.name == n));
                fv.entries = entries;
                // Keep the old previews on a refresh until the new ones come.
                if fv.previews_for != path {
                    fv.previews.clear();
                    fv.previews_for = path.clone();
                }
                fv.previews_asked.clear();
                fv.listed = Some(path);
            }
            vec![]
        }
        Msg::DropInto { paths, dest, copy } => transfer(state, &paths, !copy, &dest),
        Msg::FvNewFolder | Msg::FvNewProject => {
            let fv = &state.folder_view;
            if is_smart(&fv.path) {
                state.messages_info("Smart folders are read-only: make a collection outside smart/");
                return vec![];
            }
            let Some(tree) = fv.tree() else {
                state.messages_info("The inbox has no folders of its own");
                return vec![];
            };
            let rel = fv.tree_rel(&fv.path);
            match msg {
                Msg::FvNewProject if tree == Tree::Projects => update(state, Msg::NewProject(rel)),
                _ => update(state, Msg::NewFolder(tree, rel)),
            }
        }
        Msg::FvWantPreviews(names) => {
            let fv = &mut state.folder_view;
            let mut names: Vec<String> = names.into_iter().filter(|n| fv.previews_asked.insert(n.clone())).collect();
            // the virtual smart folders: from the index
            let dir = fv.path.clone();
            let (smart, rest): (Vec<String>, Vec<String>) = names.drain(..).partition(|n| is_smart(&dir.join(n)));
            let found: Vec<(String, Vec<ImageRef>)> = smart
                .into_iter()
                .map(|n| {
                    let rel = dir.join(&n).strip_prefix(SMART_DIR).map(PathBuf::from).unwrap_or_default();
                    let imgs = state.smart_images(&rel).into_iter().take(3).map(|o| o.image.clone()).collect();
                    (n, imgs)
                })
                .collect();
            state.folder_view.previews.extend(found);
            let names = rest;
            let fv = &state.folder_view;
            match &state.library {
                Some(root) if !names.is_empty() => {
                    vec![Effect::FolderPreviews { root: root.clone(), dir: fv.path.clone(), names }]
                }
                _ => vec![],
            }
        }
        Msg::FvPreviews(dir, found) => {
            let fv = fv_for(state, &dir);
            if dir == fv.path {
                fv.previews.extend(found);
            }
            vec![]
        }
        Msg::FvSelectSet(names) => {
            state.folder_view.selected = names;
            vec![]
        }
        Msg::FvMoveCursor { by, extend } => {
            let fv = &mut state.folder_view;
            let names: Vec<String> = fv.shown().iter().map(|e| e.name.clone()).collect();
            if names.is_empty() {
                return vec![];
            }
            let at = fv.cursor.as_ref().or(fv.anchor.as_ref()).and_then(|c| names.iter().position(|n| n == c));
            let to = match at {
                Some(i) => (i as i64 + by as i64).clamp(0, names.len() as i64 - 1) as usize,
                None => 0, // nothing yet: the first entry
            };
            let name = names[to].clone();
            if extend {
                let a = fv.anchor.as_ref().and_then(|a| names.iter().position(|n| n == a)).unwrap_or(to);
                fv.selected = names[a.min(to)..=a.max(to)].iter().cloned().collect();
            } else {
                fv.selected = [name.clone()].into();
                fv.anchor = Some(name.clone());
            }
            fv.cursor = Some(name);
            vec![]
        }
        Msg::FvTypeAhead(text) => {
            let now = state.now;
            let fv = &mut state.folder_view;
            if now - fv.typeahead_at > 1.0 {
                fv.typeahead.clear();
            }
            fv.typeahead_at = now;
            fv.typeahead.push_str(&text.to_lowercase());
            let hit =
                fv.shown().iter().find(|e| e.label().to_lowercase().starts_with(&fv.typeahead)).map(|e| e.name.clone());
            if let Some(name) = hit {
                fv.selected = [name.clone()].into();
                fv.anchor = Some(name.clone());
                fv.cursor = Some(name);
            }
            vec![]
        }
        Msg::FvSelect(name, how) => {
            let fv = &mut state.folder_view;
            fv.cursor = Some(name.clone());
            match how {
                SelectHow::Replace => {
                    fv.selected = [name.clone()].into();
                    fv.anchor = Some(name);
                }
                SelectHow::Toggle => {
                    if !fv.selected.remove(&name) {
                        fv.selected.insert(name.clone());
                    }
                    fv.anchor = Some(name);
                }
                SelectHow::Range => {
                    let names: Vec<String> = fv.shown().iter().map(|e| e.name.clone()).collect();
                    let a = fv.anchor.as_ref().and_then(|a| names.iter().position(|n| n == a)).unwrap_or(0);
                    let b = names.iter().position(|n| n == &name).unwrap_or(a);
                    let (lo, hi) = (a.min(b), a.max(b));
                    fv.selected = names[lo..=hi].iter().cloned().collect();
                }
            }
            vec![]
        }
        Msg::FvSelectAll => {
            let fv = &mut state.folder_view;
            fv.selected = fv.shown().iter().map(|e| e.name.clone()).collect();
            vec![]
        }
        Msg::FvClearSelection => {
            state.folder_view.selected.clear();
            vec![]
        }
        Msg::FvMode(m) => {
            state.folder_view.mode = m;
            vec![]
        }
        Msg::FvIconSize(v) => {
            state.folder_view.icon_size = v.clamp(64.0, 256.0);
            vec![]
        }
        Msg::FvShowMeta(on) => {
            state.folder_view.show_meta = on;
            vec![]
        }
        Msg::FvPathEdit(t) => {
            state.folder_view.path_edit = t;
            vec![]
        }
        Msg::FvOpen(name) => fv_open(state, &name),
        Msg::FvCopy | Msg::FvCut => {
            let cut = msg == Msg::FvCut;
            let fv = &state.folder_view;
            let smart = is_smart(&fv.path);
            let mut paths = vec![];
            let mut skipped = false;
            for e in fv.shown().into_iter().filter(|e| fv.selected.contains(&e.name)) {
                let ok = match (fv.place, &e.kind) {
                    // smart images can be copied into a collection
                    _ if smart => !cut && matches!(e.kind, EntryKind::Image(_)),
                    (FvPlace::Collections, k) => matches!(k, EntryKind::Folder | EntryKind::Image(_)),
                    (FvPlace::InputLibrary, k) => matches!(k, EntryKind::Folder | EntryKind::Idea(_)),
                    (FvPlace::Inbox, _) => false,
                    (_, EntryKind::Folder | EntryKind::Project) => true,
                    (_, EntryKind::Image(_)) => cut,
                    _ => false,
                };
                if ok {
                    paths.push(fv.path.join(&e.name));
                } else {
                    skipped = true;
                }
            }
            if skipped {
                state.messages_info(match (fv.place, cut) {
                    _ if smart => "Smart folders are read-only: copy their images into a collection",
                    (FvPlace::Collections, _) => "Only collections and their images can be copied or cut",
                    (FvPlace::InputLibrary, _) => "Only folders and ideas can be copied or cut",
                    (FvPlace::Inbox, _) => "Inbox ideas are saved to the input library or discarded from the Grid",
                    (_, true) => "Only folders, projects and image links can be cut",
                    (_, false) => {
                        "Only folders and projects can be copied (cut + paste moves image links between review folders)"
                    }
                });
            }
            if !paths.is_empty() {
                state.fv_clipboard = Some(FvClipboard { paths, cut });
            }
            vec![]
        }
        Msg::FvPaste(sub) => fv_paste(state, sub),
        Msg::FvDelete => {
            let fv = &state.folder_view;
            if is_smart(&fv.path) || fv.selected.iter().any(|n| is_smart(&fv.path.join(n))) {
                state.messages_info("Smart folders are read-only");
                return vec![];
            }
            let place = fv.place;
            if place == FvPlace::Inbox {
                state.messages_info("Inbox ideas are discarded from the Grid; files in inbox_drop/ are imported");
                return vec![];
            }
            let targets: Vec<PathBuf> = fv
                .shown()
                .into_iter()
                .filter(|e| {
                    fv.selected.contains(&e.name)
                        && match place {
                            FvPlace::InputLibrary => matches!(e.kind, EntryKind::Folder | EntryKind::Idea(_)),
                            _ => matches!(e.kind, EntryKind::Folder | EntryKind::Project | EntryKind::Image(_)),
                        }
                })
                .map(|e| fv.path.join(&e.name))
                .collect();
            if targets.is_empty() {
                return vec![];
            }
            let text = if place == FvPlace::InputLibrary {
                format!(
                    "Discard {} item(s)? Ideas go to Discarded (restore them from there); folders go to the trash.",
                    targets.len()
                )
            } else if place == FvPlace::Collections {
                format!(
                    "Remove {} item(s)? Collections go to the trash with their links; the images keep their rating and project.",
                    targets.len()
                )
            } else {
                format!(
                    "Delete {} item(s)? Projects go to Discarded (restore them from Projects → Discarded); images go to Output library → Discarded; folders go to the trash.",
                    targets.len()
                )
            };
            let verb = if place == FvPlace::Collections { "Remove" } else { "Delete" };
            update(state, Msg::Ask(Confirm::new(text, verb, Msg::FvDeleteConfirmed(targets))))
        }
        Msg::FvDeleteConfirmed(targets) => {
            let mut fx = vec![];
            for t in targets {
                let is_link = StoreFormat::of_path(&t).is_some();
                if let Ok(rel) = t.strip_prefix("collections") {
                    fx.extend(update(state, Msg::DeleteNode(Tree::Collections, rel.to_path_buf())));
                } else if let Ok(rel) = t.strip_prefix("input/library") {
                    fx.extend(update(state, Msg::DeleteNode(Tree::InputLibrary, rel.to_path_buf())));
                } else if is_link {
                    fx.extend(update(state, Msg::DeleteOutput(t)));
                } else if let Ok(rel) = t.strip_prefix("projects") {
                    fx.extend(update(state, Msg::DeleteNode(Tree::Projects, rel.to_path_buf())));
                }
            }
            fx
        }
        Msg::FvRename => {
            let fv = &state.folder_view;
            let sel: Vec<_> = fv.shown().into_iter().filter(|e| fv.selected.contains(&e.name)).collect();
            match sel.as_slice() {
                [e] if is_smart(&fv.path.join(&e.name)) => {
                    state.messages_info("Smart folders are read-only");
                    vec![]
                }
                [e] if matches!(e.kind, EntryKind::Idea(_)) => {
                    state.messages_info("An idea is renamed by editing its title (Enter → Edit idea)");
                    vec![]
                }
                [e] if matches!(e.kind, EntryKind::Folder | EntryKind::Project) && fv.tree().is_some() => {
                    let (tree, rel) = (fv.tree().unwrap_or(Tree::Projects), fv.tree_rel(&fv.path.join(&e.name)));
                    update(state, Msg::StartRename(tree, rel))
                }
                [_] => {
                    state.messages_info(
                        "Only folders and projects can be renamed here (image link names carry run, revision and seed)",
                    );
                    vec![]
                }
                _ => vec![],
            }
        }
        Msg::Problem(e) => {
            state.messages_error(e);
            vec![]
        }
        Msg::Info(t) => {
            state.messages_info(t);
            vec![]
        }
        Msg::LibraryChanged => match state.library {
            Some(_) => vec![Effect::Rescan],
            None => vec![],
        },
        Msg::DropReady => match state.library {
            Some(_) => {
                state.messages_info("New files in inbox_drop/: importing them as ideas");
                lib(state, LibOp::ImportInboxDrop)
            }
            None => vec![],
        },
        Msg::WatchStatus(s) => {
            state.watch_status = s;
            vec![]
        }
        Msg::DismissMessage(i) => {
            if i < state.messages.len() {
                state.messages.remove(i);
            }
            vec![]
        }
        Msg::UsageCheck(done, total) => {
            state.usage_check = (done < total).then_some((done, total));
            state.usage_checked = (done >= total).then_some((total, state.now));
            vec![]
        }
        Msg::ClearMessages => {
            state.messages.clear();
            vec![]
        }
    }
}

fn lib(state: &mut AppState, op: LibOp) -> Vec<Effect> {
    state.busy += 1;
    vec![Effect::Lib(op)]
}

/// The New idea form's draft as an idea: into the inbox, (Keep) named
/// `name` into the library folder `parent`, or (New project) the library and a
/// new project `name` in `parent`.
/// `saved_in`: New project, the library folder the idea goes in (else the
/// top level).
fn add_idea(
    state: &mut AppState,
    how: AddIdea,
    parent: PathBuf,
    name: String,
    saved_in: Option<PathBuf>,
) -> Vec<Effect> {
    let mut d = std::mem::take(&mut state.draft);
    if how == AddIdea::Keep {
        d.title = name.clone();
    }
    state.idea_dirty = None;
    let text_for_history = (d.text.clone(), d.title.clone());
    let tags = d.tag_list();
    let loc = match how {
        AddIdea::Inbox => IdeaLoc::Unreviewed,
        AddIdea::Keep => IdeaLoc::Library(parent.clone()),
        AddIdea::NewProject => saved_in.map_or_else(IdeaLoc::top_library, IdeaLoc::Library),
    };
    let idea = NewIdea { title: d.title, text: d.text, tags, images: d.images, loc, ..NewIdea::text("", "") };
    state.busy += 1;
    let add = match how {
        AddIdea::NewProject => Effect::Lib(LibOp::NewIdeaNewProject(idea, parent, name)),
        _ => Effect::Lib(LibOp::NewIdea(idea)),
    };
    state.saving += 1;
    let mut fx = vec![add, Effect::SaveDraft(DraftSave::ClearIdea)];
    let (t, x) = (text_for_history.0, text_for_history.1);
    fx.extend(record(state, HistoryKey::NewIdea, HistoryKind::Saved, t, x));
    fx
}

/// Why Save to input library can't save: an empty name (one idea or the
/// draft), or a
/// bad new folder name.
pub fn save_idea_problem(d: &SaveIdeaDialog) -> Option<String> {
    if d.ideas.len() <= 1 && d.name.trim().is_empty() {
        return Some("the idea needs a name".into());
    }
    let folder = d.new_folder.as_deref().map(str::trim)?;
    increart_core::library::validate_name(folder).err().map(|e| format!("new folder: {e}"))
}

/// Where Save to input library puts the ideas: the dialog's folder, or its
/// new folder in it (an existing one of that name, any case, is used); and
/// whether that folder exists.
pub fn save_idea_folder(state: &AppState, d: &SaveIdeaDialog) -> (PathBuf, bool) {
    let Some(new) = &d.new_folder else {
        return (d.folder.clone(), true);
    };
    let at = d.folder.join(new.trim());
    let same = |f: &&PathBuf| f.to_string_lossy().to_lowercase() == at.to_string_lossy().to_lowercase();
    match state.index.library_folders.iter().find(same) {
        Some(f) => (f.clone(), true),
        None => (at, false),
    }
}

/// Why the New project from ideas dialog can't create the project (in its folder when
/// `in_folder`): a bad or taken name, or a folder name a project has.
pub fn from_ideas_problem(state: &AppState, d: &FromIdeasDialog) -> Option<String> {
    let name = d.name.trim();
    if let Err(e) = increart_core::library::validate_name(name) {
        return Some(e.to_string());
    }
    let (at, exists) = from_ideas_folder(state, d);
    if let Some(folder) = d.new_folder.as_deref().map(str::trim) {
        if let Err(e) = increart_core::library::validate_name(folder) {
            return Some(format!("new folder: {e}"));
        }
        if !exists && state.project_name_taken(&at, folder) {
            return Some(format!("a project is named {folder} there"));
        }
    }
    state.project_name_taken(&at.join("_"), name).then(|| format!("{name} already exists there"))
}

/// Where the project goes: the dialog's folder, or its new folder in it
/// (an existing one of that name, any case, is used); and whether that
/// folder exists.
pub fn from_ideas_folder(state: &AppState, d: &FromIdeasDialog) -> (PathBuf, bool) {
    let Some(new) = &d.new_folder else {
        return (d.parent.clone(), true);
    };
    let at = d.parent.join(new.trim());
    let same = |f: &&PathBuf| f.to_string_lossy().to_lowercase() == at.to_string_lossy().to_lowercase();
    match state.index.project_folders.iter().find(same) {
        Some(f) => (f.clone(), true),
        None => (at, false),
    }
}

/// Changes this library's collapsed project folders (keeping only folders
/// that still exist) and saves the config.
fn set_collapsed(state: &mut AppState, change: impl FnOnce(&mut Vec<PathBuf>)) -> Vec<Effect> {
    let Some(key) = state.library.as_ref().map(|l| l.to_string_lossy().into_owned()) else { return vec![] };
    let folders = &state.index.project_folders;
    let mut c = state.config.collapsed_folders.remove(&key).unwrap_or_default();
    change(&mut c);
    c.retain(|f| folders.contains(f));
    c.sort();
    c.dedup();
    if !c.is_empty() {
        state.config.collapsed_folders.insert(key, c);
    }
    save_config(state)
}

/// The New project from ideas dialog's first folder: where the last one went, if it's
/// still there, else the top level.
fn from_ideas_parent(state: &AppState) -> PathBuf {
    state
        .from_ideas_last
        .clone()
        .filter(|p| p.as_os_str().is_empty() || state.index.project_folders.contains(p))
        .unwrap_or_default()
}

/// The New project from ideas dialog's first guess: the first idea title that makes a
/// valid, free project name (numbered if taken), else empty.
fn from_ideas_name<'a>(state: &AppState, titles: impl IntoIterator<Item = &'a str>) -> String {
    let at = from_ideas_parent(state).join("_");
    titles
        .into_iter()
        .map(str::trim)
        .find(|t| !t.is_empty() && increart_core::library::validate_name(t).is_ok())
        .map(|t| {
            std::iter::once(t.to_string())
                .chain((2..).map(|n| format!("{t} {n}")))
                .find(|n| !state.project_name_taken(&at, n))
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// Ctrl+click, Shift+click or a plain click on a pickable image.
fn pick_image(state: &mut AppState, image: ImageRef, how: SelectHow) {
    // picking starts from the image the cursor is on
    let start = |state: &mut AppState| {
        if state.picked.is_empty()
            && let Some(c) = state.current_image()
        {
            state.picked.push(c);
        }
    };
    match how {
        SelectHow::Replace => state.picked.clear(),
        SelectHow::Toggle => {
            start(state);
            match state.picked.iter().position(|i| i == &image) {
                Some(at) => {
                    state.picked.remove(at);
                }
                None => state.picked.push(image),
            }
        }
        SelectHow::Range => {
            start(state);
            let all = state.pickable();
            let to = all.iter().position(|i| i == &image);
            let from = state.picked.last().and_then(|a| all.iter().position(|i| i == a)).or(to);
            let (Some(from), Some(to)) = (from, to) else { return };
            let range: Vec<ImageRef> =
                if from <= to { all[from..=to].to_vec() } else { all[to..=from].iter().rev().cloned().collect() };
            for i in range {
                if !state.picked.contains(&i) {
                    state.picked.push(i);
                }
            }
        }
    }
}

/// **New sweep from these**, with the project's prompt history: the form
/// the sweep was queued with (its template, values, settings and styles'
/// own prompts), only the rows chosen ticked (a setting sweep: their
/// values), the next label and seeds, in Generate. Without that checkpoint
/// (older sweeps, a trimmed history) the first run's settings, and each
/// style its run's very prompt.
fn new_sweep(state: &mut AppState, ask: NewSweepAsk, entries: &[HistoryEntry]) -> Vec<Effect> {
    use increart_core::prompts::{PRESET_SLOT, PresetKey, StylePrompt, SweepSource, SweepVary, sweep_prompt};
    let Some(p) = state.index.project(&ask.project) else { return vec![] };
    let runs: Vec<increart_core::model::Run> =
        ask.runs.iter().filter_map(|id| p.runs.iter().find(|r| &r.id == id)).cloned().collect();
    let Some(first) = runs.first() else { return vec![] };
    let sweep_runs: Vec<&Id> =
        p.runs.iter().filter(|r| r.sweep.as_ref().is_some_and(|s| s.name == ask.sweep)).map(|r| &r.id).collect();
    let queued = entries.iter().rev().find(|e| match &e.kind {
        HistoryKind::Queued(_) if !e.runs.is_empty() => e.runs.iter().any(|r| sweep_runs.contains(&r)),
        HistoryKind::Queued(label) => *label == ask.sweep,
        _ => false,
    });
    let form = queued.and_then(|e| e.form.as_deref());
    let mut f = match form {
        Some(d) => GenerateForm::from_draft(d, &state.config),
        None => GenerateForm::from_run(first, &state.config),
    };
    let tags: Vec<&SweepTag> = runs.iter().filter_map(|r| r.sweep.as_ref()).collect();
    let param = tags.first().and_then(|t| t.param.as_ref());
    let s = &mut f.sweep;
    s.on = true;
    s.removed.clear();
    s.random = 0;
    let mut gone = vec![];
    match param {
        Some(param) => {
            s.vary = SweepVary::EVERY.into_iter().find(|v| v.key() == param.name).unwrap_or_default();
            let values: Vec<String> = tags.iter().filter_map(|t| t.param.as_ref()).map(|p| p.value.clone()).collect();
            if s.vary == SweepVary::Model {
                s.models = values;
            } else {
                s.values = values.join(", ");
            }
        }
        None => {
            s.vary = SweepVary::Style;
            s.source = SweepSource::Selected;
            let list = &state.prompt_presets;
            let keys: Vec<PresetKey> = tags
                .iter()
                .map(|t| {
                    increart_core::prompts::current_key(
                        &PresetKey { task: t.task.clone(), name: t.preset.clone() },
                        list,
                    )
                })
                .collect();
            let find = |k: &PresetKey| {
                list.tasks.iter().find(|t| t.name == k.task).and_then(|t| t.presets.iter().find(|p| p.name == k.name))
            };
            for k in &keys {
                if find(k).is_none() {
                    gone.push(k.name.clone());
                }
            }
            s.selected = keys.clone();
            s.style_prompts.retain(|sp| keys.contains(&sp.preset));
            if form.is_none() {
                // no template: each style its run's own prompt
                f.prompt = PRESET_SLOT.into();
                let slots = f.sweep.slots.clone();
                let mut own = vec![];
                for (run, key) in runs.iter().zip(&keys) {
                    let Some(preset) = find(key) else { continue };
                    let enhanced = run.sweep.as_ref().is_some_and(|t| t.enhanced);
                    let from = sweep_prompt(PRESET_SLOT, &preset.prompt, &slots);
                    own.push(StylePrompt { preset: key.clone(), prompt: run.prompt.clone(), from, enhanced });
                }
                f.sweep.style_prompts = own;
            }
        }
    }
    let n = runs.len();
    let mut fx = update(state, Msg::OpenInGenerate(ask.project.clone()));
    let g = &state.generate;
    if !g.prompt.trim().is_empty() {
        let (t, x) = (g.prompt.clone(), g.negative.clone());
        fx.extend(record(state, HistoryKey::Project(ask.project), HistoryKind::BeforeRestore, t, x));
    }
    f.label = next_label(state);
    f.first_seed = state.next_seed();
    f.sweep.suggestions = std::mem::take(&mut state.generate.sweep.suggestions);
    f.sweep.batch_ask = state.generate.sweep.batch_ask.take();
    f.sweep.enhance_ask = state.generate.sweep.enhance_ask.take();
    state.generate = f;
    mark_generate(state);
    let what = if param.is_some() { "values" } else { "styles" };
    let mut note = format!("Sweep {} again: {n} {what} in Generate, to queue", ask.sweep);
    if form.is_none() {
        note.push_str("; its template wasn't in the prompt history, so each style keeps its run's prompt");
    }
    if !gone.is_empty() {
        note.push_str(&format!("; no longer in the presets, left out: {}", gone.join(", ")));
    }
    state.messages_info(note);
    fx
}

/// Leaving a sweep (or run) scope: the sheet goes with it.
fn leave_sweep(state: &mut AppState) {
    state.review_sweep = None;
    state.review_run = None;
    state.picked.clear();
    state.sheet_filter.clear();
    if state.review_view == ReviewView::Sheet {
        state.review_view = ReviewView::Single;
    }
}

/// `r00`, `r01`…: one number per run, a sweep counting once, and past the
/// highest `rNN` already there (libraries from before started at `r01`).
fn next_label(state: &AppState) -> String {
    let n = state.project_entry().map_or(0, |p| {
        let sweeps: std::collections::HashSet<_> =
            p.runs.iter().filter_map(|r| r.sweep.as_ref()).map(|s| &s.name).collect();
        let count = p.runs.iter().filter(|r| r.sweep.is_none()).count() + sweeps.len();
        let number = |label: &str| {
            let digits: String = label.strip_prefix('r')?.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<usize>().ok()
        };
        let past = p.runs.iter().filter_map(|r| number(&r.label)).max().map_or(0, |m| m + 1);
        count.max(past)
    });
    format!("r{n:02}")
}

/// Images in a sweep before Queue asks first.
/// The error an archive thread answers when it was stopped.
pub const ARCHIVE_STOPPED: &str = "stopped";

pub const BIG_SWEEP: u64 = 100;

/// The Sweep mode's runs, checked: (form, tag) per chosen preset that the
/// references fit, and how many chosen presets need more references.
pub fn sweep_runs(state: &AppState) -> Result<(Vec<(GenerateForm, SweepTag)>, usize), String> {
    use increart_core::prompts::{slot_names, sweep_presets};
    let f = &state.generate;
    if f.sweep.vary != increart_core::prompts::SweepVary::Style {
        return param_runs(f, &state.config.recipes).map(|r| (r, 0));
    }
    let refs = f.refs().len();
    let chosen = sweep_presets(&state.prompt_presets, &f.sweep);
    let needs = |p: &str| image_tags(p).into_iter().map(|(_, n)| n).max().unwrap_or(0);
    let (fit, unfit): (Vec<_>, Vec<_>) = chosen.into_iter().partition(|(_, p)| needs(&p.prompt) <= refs);
    if fit.is_empty() {
        return Err(if unfit.is_empty() {
            "No styles chosen for the sweep".into()
        } else {
            format!("None of the chosen presets fit {refs} reference image(s)")
        });
    }
    // a blank is filled for a style by its own value or the shared one
    let key = |t: &increart_core::prompts::PromptTask, p: &increart_core::prompts::PromptPreset| {
        increart_core::prompts::PresetKey { task: t.name.clone(), name: p.name.clone() }
    };
    // (a style's own prompt is used as it is, any blank left filled too)
    let mut unfilled: Vec<String> = vec![];
    for (t, p) in &fit {
        for n in slot_names([f.sweep.prompt_for(&f.prompt, &key(t, p), &p.prompt).as_str()]) {
            let n = format!("{{{n}}}");
            if !unfilled.contains(&n) {
                unfilled.push(n);
            }
        }
    }
    if !unfilled.is_empty() {
        return Err(format!("Fill in {} for the sweep first", unfilled.join(", ")));
    }
    let name = f.label.trim().to_string();
    let runs = fit
        .into_iter()
        .map(|(t, p)| {
            let mut run = f.clone();
            run.sweep = Default::default();
            run.prompt = f.sweep.prompt_for(&f.prompt, &key(t, p), &p.prompt);
            if !p.negative.is_empty() {
                run.negative = p.negative.clone();
            }
            run.label = format!("{name} · {}", p.name);
            let enhanced = f.sweep.style_prompt(&key(t, p)).is_some_and(|s| s.enhanced);
            let tag =
                SweepTag { name: name.clone(), task: t.name.clone(), preset: p.name.clone(), param: None, enhanced };
            (run, tag)
        })
        .collect();
    Ok((runs, unfit.len()))
}

/// The `<imageN>` numbers the queued prompts would name: the prompt's,
/// and with a sweep on each style's prompt (its own, or the template with
/// the style in) or the prompt with its blanks filled.
pub fn named_image_tags(state: &AppState) -> std::collections::BTreeSet<usize> {
    use increart_core::prompts::{PresetKey, SweepVary, fill_slots, sweep_presets};
    let f = &state.generate;
    let mut prompts = vec![f.prompt.clone()];
    if f.sweep.on {
        if f.sweep.vary == SweepVary::Style {
            for (t, p) in sweep_presets(&state.prompt_presets, &f.sweep) {
                let key = PresetKey { task: t.name.clone(), name: p.name.clone() };
                prompts.push(f.sweep.prompt_for(&f.prompt, &key, &p.prompt));
            }
        } else {
            prompts.push(fill_slots(f.prompt.clone(), &f.sweep.slots));
        }
    } else {
        prompts.push(fill_slots(f.prompt.clone(), &f.sweep.slots));
    }
    prompts.iter().flat_map(|p| image_tags(p)).map(|(_, n)| n).collect()
}

/// The references (their `<imageN>` numbers) none of the queued prompts
/// name, when there are several: likely a tag forgotten, so Queue asks.
/// One reference on its own is commonly left unnamed.
pub fn unnamed_refs(state: &AppState) -> Vec<usize> {
    let n = state.generate.refs().len();
    if n < 2 {
        return vec![];
    }
    let named = named_image_tags(state);
    (1..=n).filter(|i| !named.contains(i)).collect()
}

/// A setting sweep's runs: one per value, the setting changed and the rest
/// as the form has it (a model sweep takes each model preset's own steps,
/// CFG and flow shift).
pub fn param_runs(f: &GenerateForm, recipes: &[RecipePreset]) -> Result<Vec<(GenerateForm, SweepTag)>, String> {
    use increart_core::prompts::{PRESET_SLOT, SweepVary, fill_slots};
    let values = sweep_values(f, recipes)?;
    if f.prompt.contains(PRESET_SLOT) {
        return Err(format!("{PRESET_SLOT} in the prompt is for style sweeps: take it out, or vary the style"));
    }
    let prompt = fill_slots(f.prompt.clone(), &f.sweep.slots);
    let unfilled: Vec<String> =
        increart_core::prompts::slot_names([prompt.as_str()]).into_iter().map(|n| format!("{{{n}}}")).collect();
    if !unfilled.is_empty() {
        return Err(format!("Fill in {} (type over it) before queueing", unfilled.join(", ")));
    }
    let name = f.label.trim().to_string();
    let runs = values
        .into_iter()
        .map(|param| {
            let mut run = f.clone();
            run.sweep = Default::default();
            run.prompt = prompt.clone();
            let v: f32 = param.value.parse().unwrap_or_default();
            match f.sweep.vary {
                SweepVary::Steps => run.steps = v as u32,
                SweepVary::Cfg => run.cfg = v,
                SweepVary::FlowShift => run.flow_shift = Some(v),
                SweepVary::Strength => run.strength = v,
                SweepVary::Model => {
                    if let Some(i) = recipes.iter().position(|r| r.name == param.value) {
                        let p = &recipes[i];
                        (run.preset, run.steps, run.cfg, run.flow_shift) = (i, p.steps, p.cfg, p.flow_shift);
                    }
                }
                SweepVary::Style => {}
            }
            run.label = format!("{name} · {}", param.label());
            let tag = SweepTag {
                name: name.clone(),
                task: String::new(),
                preset: String::new(),
                param: Some(param),
                enhanced: false,
            };
            (run, tag)
        })
        .collect();
    Ok(runs)
}

/// A setting sweep's values, from what was typed (in order, without
/// repeats) or the ticked models, or what's wrong with them.
pub fn sweep_values(f: &GenerateForm, recipes: &[RecipePreset]) -> Result<Vec<SweepParam>, String> {
    use increart_core::prompts::SweepVary;
    let vary = f.sweep.vary;
    let param = |value: String| SweepParam { name: vary.key().into(), value };
    if vary == SweepVary::Model {
        let v: Vec<_> =
            recipes.iter().filter(|r| f.sweep.models.contains(&r.name)).map(|r| param(r.name.clone())).collect();
        return if v.is_empty() { Err("Tick the models to compare".into()) } else { Ok(v) };
    }
    if vary == SweepVary::Strength && f.init().is_none() {
        return Err("A strength sweep needs a start image".into());
    }
    let (lo, hi) = match vary {
        SweepVary::Steps => (1.0, 200.0),
        SweepVary::Cfg => (0.0, 30.0),
        SweepVary::FlowShift => (0.0, 20.0),
        _ => (0.0, 1.0),
    };
    let mut v: Vec<f32> = vec![];
    for w in f.sweep.values.split(|c: char| c == ',' || c == ';' || c.is_whitespace()).filter(|w| !w.is_empty()) {
        let x: f32 = w.parse().map_err(|_| format!("“{w}” isn't a number"))?;
        if vary == SweepVary::Steps && x.fract() != 0.0 {
            return Err(format!("Steps are whole numbers: {w}"));
        }
        // CFG may be 0 … 30 and steps 1 … 200; the rest above 0
        let low_ok = if vary == SweepVary::Steps || vary == SweepVary::Cfg { x >= lo } else { x > lo };
        if !low_ok || x > hi {
            return Err(format!("{} goes from {lo} to {hi}: {w}", vary.name()));
        }
        v.push(x);
    }
    v.sort_by(f32::total_cmp);
    v.dedup();
    if v.is_empty() {
        return Err("Type the values to compare, e.g. 8, 12, 20".into());
    }
    Ok(v.into_iter().map(|x| param(fmt_value(x))).collect())
}

/// A setting value as typed: "2", "2.5", "0.45".
pub fn fmt_value(x: f32) -> String {
    format!("{}", (x * 1000.0).round() / 1000.0)
}

/// `from`, `from + step`, … up to `to` (at most 50), for the values field.
pub fn range_values(from: f32, to: f32, step: f32) -> String {
    if step <= 0.0 || to < from {
        return fmt_value(from);
    }
    let n = (((to - from) / step + 1e-4).floor() as usize + 1).min(50);
    (0..n).map(|i| fmt_value(from + step * i as f32)).collect::<Vec<_>>().join(", ")
}

/// A setting sweep's starting values, around the form's own.
pub fn sweep_defaults(vary: increart_core::prompts::SweepVary, f: &GenerateForm) -> String {
    use increart_core::prompts::SweepVary;
    let round = |x: f32, to: f32| (x / to).round() * to;
    let v: Vec<f32> = match vary {
        SweepVary::Steps => {
            let s = f.steps.max(2) as f32;
            [0.5, 0.75, 1.0, 1.5, 2.0].iter().map(|k| (s * k).round().max(1.0)).collect()
        }
        SweepVary::Cfg => {
            let c = f.cfg.max(1.0);
            [0.5, 0.75, 1.0, 1.5, 2.0].iter().map(|k| round(c * k, 0.5).max(1.0)).collect()
        }
        SweepVary::FlowShift => {
            let x = f.flow_shift.unwrap_or(3.0);
            [0.5, 2.0 / 3.0, 1.0, 4.0 / 3.0, 2.0].iter().map(|k| round(x * k, 0.1)).collect()
        }
        SweepVary::Strength => vec![0.3, 0.45, 0.6, 0.75, 0.9],
        SweepVary::Style | SweepVary::Model => vec![],
    };
    let mut v = v;
    v.dedup();
    v.into_iter().map(fmt_value).collect::<Vec<_>>().join(", ")
}

/// A run or sweep of this label is still being saved (Queue pressed
/// again before it was, as with a double-click): said, and not saved twice.
fn still_saving(state: &mut AppState, project: &Id, label: &str) -> bool {
    let saving = state.queueing.iter().any(|(p, l)| p == project && l == label);
    if saving {
        state.messages_info(format!("{} is being queued already", label.trim()));
    }
    saving
}

fn queue_sweep(state: &mut AppState, confirmed: bool) -> Vec<Effect> {
    let Some(project) = state.project.clone() else {
        state.messages_error("Select a project first");
        return vec![];
    };
    let f = &state.generate;
    if !f.width.is_multiple_of(32) || !f.height.is_multiple_of(32) || f.seeds == 0 {
        state.messages_error("Size must be a multiple of 32 and seeds at least 1");
        return vec![];
    }
    let name = f.label.trim();
    if name.is_empty() {
        state.messages_error("Give the sweep a label");
        return vec![];
    }
    let taken = state
        .project_entry()
        .is_some_and(|p| p.runs.iter().any(|r| r.sweep.as_ref().is_some_and(|s| s.name == name) || r.label == name));
    if taken {
        state.messages_error(format!("This project already has a run or sweep labelled {name}"));
        return vec![];
    }
    let (runs, _) = match sweep_runs(state) {
        Ok(r) => r,
        Err(e) => {
            state.messages_error(e);
            return vec![];
        }
    };
    // each run's own model preset (a model sweep has several)
    let Some(runs) = runs
        .into_iter()
        .map(|(f, t)| {
            let preset = state.config.recipes.get(f.preset).cloned()?;
            Some((f, t, preset))
        })
        .collect::<Option<Vec<_>>>()
    else {
        state.messages_error("No model preset");
        return vec![];
    };
    let images = runs.len() as u64 * state.generate.seeds;
    if images > BIG_SWEEP && !confirmed {
        let text = format!("Queue {} runs × {} seeds = {images} images?", runs.len(), state.generate.seeds);
        state.confirm = Some(Confirm::new(text, "Queue", Msg::QueueSweep(true)));
        return vec![];
    }
    // one run at a time would fill the contact sheet a style at a time
    if state.config.job_order == Order::Sequential && !state.sweep_order_asked {
        state.sweep_order_ask = Some(confirmed);
        return vec![];
    }
    if state.servers.is_empty() && state.background.is_none() && !state.no_server_warned {
        state.no_server_warned = true;
        state.no_server_warning = true;
        return vec![];
    }
    if still_saving(state, &project, &state.generate.label.clone()) {
        return vec![];
    }
    let queued = Box::new(Queued { project: project.clone(), form: Box::new(state.generate.to_draft(&state.config)) });
    state.queueing.push((project.clone(), state.generate.label.clone()));
    let op = LibOp::CreateSweep {
        project,
        runs,
        model_dirs: state.config.programs.sd_server.model_dirs.clone(),
        queue: state.engine_running,
        queued,
    };
    if !state.engine_running {
        state.messages_info("Sweep saved; it will be generated once servers are connected");
    }
    lib(state, op)
}

/// Compare with…'s pick: A in the Output library's Grid compare, or
/// Review's A/B compare (B stays the image it was opened on).
/// The Tile dialog on this layout (`start`: Generate's start image).
fn open_tile(state: &mut AppState, layout: crate::tile::TileLayout, start: Option<ImageRef>) {
    state.tile = Some(TileDialog { layout, selected: None, working: 0, start });
}

fn compare_against(state: &mut AppState, a: ImageRef) -> Vec<Effect> {
    if state.tab == Tab::OutputLibrary {
        state.output_tab = OutputTab::Grid;
        state.output_compare = Some(a);
    } else {
        state.tab = Tab::Review;
        state.review_view = ReviewView::Compare;
        state.review_pinned = Some(a);
    }
    vec![]
}

/// Following stops once Review is moved to another image by hand.
fn unfollow(state: &mut AppState) {
    state.review_follow = false;
}

/// Follow starts its time on the image now at the cursor.
fn follow_shown(state: &mut AppState) {
    if state.review_follow {
        state.follow.shown = state.review_queue().get(state.review_cursor).map(|o| o.image.clone());
        state.follow.elapsed = 0.0;
        state.follow.at = state.now;
        state.follow.moved = None;
    }
}

/// Follow, after each change: notes new images, keeps to the image shown
/// (or the one in its place, once it's rated), and moves on once its time
/// is up and there's another after it.
fn follow_step(state: &mut AppState) {
    let now = state.now;
    let queue: Vec<ImageRef> = state.review_queue().iter().map(|o| o.image.clone()).collect();
    let f = &mut state.follow;
    let fresh = queue.iter().filter(|i| f.newest.as_ref().is_none_or(|n| &i.id > n)).count();
    if fresh > 0 {
        f.arrivals.extend(std::iter::repeat_n(now, fresh));
        f.newest = queue.iter().map(|i| i.id.clone()).max();
    }
    f.arrivals.retain(|t| now - t <= Follow::WINDOW && *t <= now);
    match f.shown.as_ref().and_then(|s| queue.iter().position(|i| i == s)) {
        Some(i) => state.review_cursor = i,
        None => {
            state.review_cursor = state.review_cursor.min(queue.len().saturating_sub(1));
            f.shown = queue.get(state.review_cursor).cloned();
            f.elapsed = 0.0;
        }
    }
    let away = f.paused || f.held || state.tab != Tab::Review || state.review_view == ReviewView::Sheet;
    if !away {
        f.elapsed += (now - f.at).max(0.0);
    }
    f.at = now;
    let ahead = queue.len().saturating_sub(state.review_cursor + 1);
    if ahead > 0 && f.elapsed >= state.config.review_follow.secs(ahead, f.interval(now)) {
        f.moved = f.shown.take().map(|i| (now, i));
        state.review_cursor += 1;
        f.shown = queue.get(state.review_cursor).cloned();
        f.elapsed = 0.0;
    }
}

fn set_index(state: &mut AppState, ix: Index) {
    state.index = ix;
    state.rebuild_usage();
    state.folder_view.listed = None;
    for v in &mut state.parked_folder_views {
        v.listed = None;
    }
    state.inbox_cursor = state.inbox_cursor.min(state.inbox_queue().len().saturating_sub(1));
    state.review_cursor = state.review_cursor.min(state.review_queue().len().saturating_sub(1));
    if state.project.as_ref().is_some_and(|p| state.index.project(p).is_none()) {
        state.project = None;
    }
    state.lib_selected.retain(|id| state.index.idea(id).is_some());
    let queue: Vec<Id> = state.inbox_queue().iter().map(|e| e.idea.id.clone()).collect();
    state.inbox_picked.retain(|id| queue.contains(id));
}

/// The Review / Output library actions that put an output image into a
/// project's Generate form (DESIGN.md, Outputs as inputs).
fn use_image(state: &mut AppState, image: ImageRef, target: UseTarget) -> Vec<Effect> {
    let Some(link) = state.output_link(&image).cloned() else { return vec![] };
    let source = state.project_of_link(&link).cloned();
    let run = state.index.run(&link.name.run).map(|(_, r)| r.clone());
    match target {
        UseTarget::CurrentProject => {
            let Some(p) = state.project_entry().cloned() else {
                state.messages_error("Select a project first (Projects tab)");
                return vec![];
            };
            let before = state.generate.refs();
            state.generate.add_reference(image.clone());
            let tag =
                state.generate.images.iter().position(|i| i.image == image).and_then(|i| state.generate.tag_of(i));
            state.messages_info(format!(
                "Added to {}'s Generate form as <image{}>",
                p.name(),
                tag.map_or("?".into(), |t| t.to_string())
            ));
            images_changed(state, before)
        }
        UseTarget::Edit => {
            let Some(p) = source else { return vec![] };
            if state.project.as_ref() != Some(&p.project.id)
                && ask_leaving_generate(state, &Msg::UseImage(image.clone(), UseTarget::Edit))
            {
                return vec![];
            }
            let mut fx = vec![];
            // the project's values offered for its sweep's blanks stay
            let sw = &state.generate.sweep;
            let mut kept = (sw.suggestions.clone(), sw.batch_ask.clone(), sw.enhance_ask.clone());
            if state.project.as_ref() != Some(&p.project.id) {
                fx = leave_generate(state);
                state.project = Some(p.project.id.clone());
                state.proj_folder = None;
                kept = p
                    .draft
                    .as_ref()
                    .map(|d| (d.sweep.suggestions.clone(), d.sweep.batch_ask.clone(), d.sweep.enhance_ask.clone()))
                    .unwrap_or_default();
            }
            let mut f = GenerateForm::new(&state.config);
            (f.sweep.suggestions, f.sweep.batch_ask, f.sweep.enhance_ask) = kept;
            f.label = next_label(state);
            f.first_seed = state.next_seed();
            if let Some(r) = &run {
                (f.width, f.height) = (r.width, r.height);
            }
            f.add_reference(image);
            f.prompt = "<image1> ".into();
            state.generate = f;
            mark_generate(state);
            state.tab = Tab::Projects;
            state.projects_tab = ProjectsTab::Generate;
            fx
        }
        UseTarget::NewProject | UseTarget::DuplicateProject => {
            let Some(p) = source else { return vec![] };
            let name = format!("{} from s{:04}", p.name(), link.name.seed);
            state.new_project = Some(NewProjectDialog { image, target, name });
            vec![]
        }
    }
}

/// Makes the project the New project / Duplicate dialog named.
fn prepare_project(state: &mut AppState, image: ImageRef, target: UseTarget, name: String) -> Vec<Effect> {
    let Some(link) = state.output_link(&image).cloned() else { return vec![] };
    let source = state.project_of_link(&link).cloned();
    let run = state.index.run(&link.name.run).map(|(_, r)| r.clone());
    let Some(p) = source else { return vec![] };
    let rel = p.dir.strip_prefix("projects").map(PathBuf::from).unwrap_or_default();
    let parent = rel.parent().map(PathBuf::from).unwrap_or_default();
    let (form, copy_of) = if target == UseTarget::NewProject {
        let mut f = GenerateForm::new(&state.config);
        if let Some(r) = &run {
            (f.prompt, f.width, f.height) = (r.prompt.clone(), r.width, r.height);
        }
        f.add_reference(image);
        (f, None)
    } else {
        let mut f = run
            .as_ref()
            .map(|r| GenerateForm::from_run(r, &state.config))
            .unwrap_or_else(|| GenerateForm::new(&state.config));
        // the run's images as they were, and this one as the next reference
        f.add_reference(image);
        (f, Some(rel))
    };
    lib(state, LibOp::PrepareProject { parent, name, copy_of, form, then: StartRefs::None })
}

/// One chat request to the PE server.
#[derive(Clone, Debug, PartialEq)]
pub struct PeJob {
    pub id: u64,
    pub key: PeKey,
    pub system: String,
    pub sampling: increart_core::pe::Sampling,
    pub text: String,
    /// Attached in this order (absolute).
    pub images: Vec<PathBuf>,
    /// Their longest side, as sent.
    pub image_size: u32,
    pub max_tokens: u32,
}

/// What the rewrite / describe window does with its answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeUse {
    Discard,
    /// Replace the text (the Generate prompt for a description), the old
    /// one kept in its history.
    Use,
    /// Keep it as one of the idea's variants.
    Variant,
}

/// Asks for the rewrite / describe window's text (again).
fn ask_text(state: &mut AppState) -> Vec<Effect> {
    use increart_core::pe::{DESCRIBE, REWRITE, TextAsk, text_message};
    let Some(server) = state.pe.server.as_ref().cloned() else {
        state.pe.text = None;
        state.messages_error(NO_CHAT);
        return vec![];
    };
    let Some(t) = state.pe.text.as_ref() else { return vec![] };
    let Some(root) = state.library.clone() else { return vec![] };
    let f = &state.generate;
    let idea = "idea (a prompt draft, notes, or both)";
    let inputs = |ids: &[Id]| ids.iter().cloned().map(ImageRef::input).collect::<Vec<_>>();
    let (mut refs, mut start, what) = match &t.target {
        PeTarget::Prompt => {
            let template = f.sweep.on && f.prompt.contains(increart_core::prompts::PRESET_SLOT);
            let what = if template { "prompt template ({preset} is where each style's prompt goes)" } else { "prompt" };
            (f.refs().to_vec(), f.init().filter(|i| !f.refs().contains(i)), what)
        }
        PeTarget::NewIdea => (inputs(&state.draft.images), None, idea),
        PeTarget::Idea(id) => {
            let ids = match &state.idea_edit {
                Some(ed) if ed.id == *id => ed.images.clone(),
                _ => state.idea(id).map(|e| e.idea.images.clone()).unwrap_or_default(),
            };
            (inputs(&ids), None, idea)
        }
        PeTarget::Describe(img) => (vec![img.clone()], None, ""),
    };
    let describe = matches!(t.target, PeTarget::Describe(_));
    if !server.vision() {
        if describe {
            state.pe.text = None;
            state.messages_error(format!("{}: describing an image needs a model with vision (mmproj)", server.name));
            return vec![];
        }
        (refs, start) = (vec![], None);
    }
    let ask = TextAsk {
        original: t.original.clone(),
        what: what.into(),
        how: t.how.clone(),
        images: refs.len(),
        start: start.is_some(),
    };
    let images: Vec<PathBuf> = refs.iter().chain(&start).map(|r| r.path(&root)).collect();
    let job = PeJob {
        id: 0,
        key: PeKey::Text(0),
        system: state.config.pe.system_prompt(if describe { DESCRIBE } else { REWRITE }).to_string(),
        sampling: state.config.pe.sampling(if describe { DESCRIBE } else { REWRITE }),
        text: text_message(&ask),
        max_tokens: 700,
        images,
        image_size: increart_core::pe::TEXT_IMAGE_SIZE,
    };
    // a new ask replaces the window's last one
    let mut fx = stop_text_ask(state);
    let job = queue_job(state, job);
    if let Some(t) = &mut state.pe.text {
        (t.waiting, t.error, t.images) = (Some(job.id), None, job.images.len());
    }
    fx.push(Effect::PeAsk { servers: pool(state, server), jobs: vec![job] });
    fx
}

/// Stops the rewrite / describe window's ask, if it's waiting.
fn stop_text_ask(state: &mut AppState) -> Vec<Effect> {
    match state.pe.text.as_ref().and_then(|t| t.waiting) {
        Some(id) => update(state, Msg::PeStop(vec![id])),
        None => vec![],
    }
}

/// Numbers an ask and queues it (a text ask's key is its number).
fn queue_job(state: &mut AppState, mut job: PeJob) -> PeJob {
    state.pe.asks += 1;
    job.id = state.pe.asks;
    if let PeKey::Text(n) = &mut job.key {
        *n = job.id;
    }
    let now = state.now;
    state.pe.jobs.push(PeJobState {
        id: job.id,
        key: job.key.clone(),
        step: PeStep::Queued,
        asked: now,
        since: now,
        rate: None,
        on: None,
    });
    job
}

/// Times a blank is asked again after an answer with nothing new.
pub const BLANK_RETRIES: u8 = 2;
/// How much hotter each time.
const RETRY_HOTTER: f32 = 0.2;
/// Times a style is asked again after an empty or unchanged answer.
pub const STYLE_RETRIES: u8 = 1;
/// Added to a style's instructions when asked again.
/// How often a writing ask's tokens a second is worked out again.
const RATE_EVERY: f64 = 0.5;
const STYLE_RETRY_NOTE: &str = "The last answer was empty or unchanged: write an improved prompt.";
/// Added to the instructions when asked again.
const RETRY_NOTE: &str = "The last answer had nothing new: give different values.";

/// Queues an ask for a blank's values, noting how it was asked (an
/// answer with nothing new asks again) and dropping a failure shown.
fn queue_blank(state: &mut AppState, job: PeJob, form: Option<&SuggestForm>, retry: u8) -> PeJob {
    if let PeKey::Blank(k) = &job.key {
        state.pe.blank_failed.remove(k);
        state.pe.blank_tries.insert(k.clone(), BlankTry { form: form.cloned(), retry });
    }
    queue_job(state, job)
}

/// Uses the rewrite / describe window's answer.
fn use_text(state: &mut AppState, t: PeText, how: PeUse) -> Vec<Effect> {
    let variant = |state: &AppState, id: &Id| {
        let server = state.pe.server.as_ref().cloned().unwrap_or_default();
        let v = increart_core::model::PromptVariant {
            text: t.text.clone(),
            origin: PeText::origin(&server, &state.config.pe, false),
        };
        LibOp::AddVariant(id.clone(), v)
    };
    match &t.target {
        PeTarget::Prompt | PeTarget::Describe(_) => {
            let mut fx = vec![];
            let f = &state.generate;
            if let Some(project) = state.project.clone()
                && !f.prompt.trim().is_empty()
            {
                let (p, n) = (f.prompt.clone(), f.negative.clone());
                fx = record(state, HistoryKey::Project(project), HistoryKind::BeforeRewrite, p, n);
            }
            state.generate.prompt = t.text;
            mark_generate(state);
            if matches!(t.target, PeTarget::Describe(_)) {
                state.messages_info("The description is the Generate prompt now");
            }
            fx
        }
        PeTarget::NewIdea => {
            let (x, title) = (state.draft.text.clone(), state.draft.title.clone());
            let mut fx = record(state, HistoryKey::NewIdea, HistoryKind::BeforeRewrite, x, title);
            let mut d = state.draft.clone();
            d.text = t.text;
            fx.extend(update(state, Msg::Draft(d)));
            fx
        }
        PeTarget::Idea(id) => {
            let open = state.idea_edit.as_ref().is_some_and(|ed| ed.id == *id);
            if how == PeUse::Variant || !open {
                if !open && how == PeUse::Use {
                    state.messages_info("The idea was closed: the rewrite is one of its variants");
                }
                let op = variant(state, id);
                return lib(state, op);
            }
            let Some(ed) = state.idea_edit.clone() else { return vec![] };
            let mut fx = record(
                state,
                HistoryKey::Idea(id.clone()),
                HistoryKind::BeforeRewrite,
                ed.text.clone(),
                ed.title.clone(),
            );
            fx.extend(update(state, Msg::EditIdea(IdeaEdit { text: t.text, ..ed })));
            fx
        }
    }
}

/// At most this many values offered before are named in an ask, not to
/// offer again.
const MAX_AVOID: usize = 60;

/// What asking prompt enhance with no chat server in use says.
const NO_CHAT: &str =
    "No chat server in use: add one to a server preset (Servers…), or pick one to continue without sd-servers";

/// The GPUs to tick in the chat server settings, listed when it runs on
/// GPUs and they aren't listed yet (or were listed by another runtime).
fn list_gpus_for_editor(state: &mut AppState) -> Vec<Effect> {
    match draft_runtime(state) {
        Some(rt) if state.pe.gpus.is_none() || state.pe.gpus_runtime != rt => update(state, Msg::PeListGpus(rt)),
        _ => vec![],
    }
}

/// The GPU runtime of the PE server being edited, when it runs on GPUs.
fn draft_runtime(state: &AppState) -> Option<increart_core::pe::GpuRuntime> {
    match &state.pe.editor.as_ref()?.draft.run {
        PeRun::KoboldCpp { device: increart_core::pe::PeDevice::Gpu { runtime, .. }, .. } => Some(*runtime),
        _ => None,
    }
}

/// A new PE server when there's none to copy: KoboldCpp on every GPU.
fn new_pe_server(runtime: increart_core::pe::GpuRuntime) -> increart_core::pe::PeServer {
    increart_core::pe::PeServer {
        name: "Prompt enhance".into(),
        run: PeRun::KoboldCpp {
            binary: String::new(),
            model: String::new(),
            mmproj: String::new(),
            config: String::new(),
            host: "127.0.0.1".into(),
            port: 5001,
            device: increart_core::pe::PeDevice::Gpu { devices: vec![], runtime },
            context: 8192,
            args: vec![],
        },
        ..Default::default()
    }
}

/// The sweep's styles that will run (the references fit them), as keys
/// with their preset.
pub fn running_styles(
    state: &AppState,
) -> Vec<(increart_core::prompts::PresetKey, &increart_core::prompts::PromptPreset)> {
    use increart_core::prompts::{PresetKey, sweep_presets};
    let f = &state.generate;
    let refs = f.refs().len();
    sweep_presets(&state.prompt_presets, &f.sweep)
        .into_iter()
        .filter(|(_, p)| crate::state::needs_refs(&p.prompt) <= refs)
        .map(|(t, p)| (PresetKey { task: t.name.clone(), name: p.name.clone() }, p))
        .collect()
}

/// A style's prompt as built from the template, the style and the values.
pub fn built_style_prompt(state: &AppState, key: &increart_core::prompts::PresetKey, style: &str) -> String {
    let f = &state.generate;
    increart_core::prompts::sweep_prompt(&f.prompt, style, &f.sweep.slots_for(key))
}

/// Where a style's row is: as built, or its own prompt and whether that
/// was made from older values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleRow {
    Built,
    Enhanced { stale: bool },
    Edited { stale: bool },
}

pub fn style_row(state: &AppState, key: &increart_core::prompts::PresetKey, style: &str) -> StyleRow {
    match state.generate.sweep.style_prompt(key) {
        None => StyleRow::Built,
        Some(own) => {
            let stale = own.from != built_style_prompt(state, key, style);
            if own.enhanced { StyleRow::Enhanced { stale } } else { StyleRow::Edited { stale } }
        }
    }
}

/// The styles **Enhance style prompts** asks for: of those that will run,
/// those not enhanced yet (as built, or enhanced from older values; not
/// the user's edits).
pub fn styles_to_enhance(state: &AppState) -> Vec<increart_core::prompts::PresetKey> {
    running_styles(state)
        .into_iter()
        .filter(|(k, p)| matches!(style_row(state, k, &p.prompt), StyleRow::Built | StyleRow::Enhanced { stale: true }))
        .map(|(k, _)| k)
        .collect()
}

/// **Enhance style prompts…**'s **Tick only** sets, of the styles that
/// will run: (not enhanced yet, failed).
pub fn enhance_picks(state: &AppState) -> [Vec<increart_core::prompts::PresetKey>; 2] {
    let failed = running_styles(state).into_iter().map(|(k, _)| k).filter(|k| state.pe.style_failed.contains_key(k));
    [styles_to_enhance(state), failed.collect()]
}

/// The message an ask for a style's prompt would send (the Enhance…
/// dialog's preview).
pub fn style_message_for(
    state: &AppState,
    key: &increart_core::prompts::PresetKey,
    ask: &EnhanceAsk,
) -> Option<String> {
    let server = state.pe.server.as_ref()?;
    style_job(state, server, key, ask).map(|(j, _, _)| j.text)
}

/// Asks to improve these styles' prompts (those not waiting already),
/// noting how (an empty or unchanged answer asks again) and dropping a
/// failure shown.
fn ask_styles(
    state: &mut AppState,
    keys: &[increart_core::prompts::PresetKey],
    ask: &EnhanceAsk,
    retry: u8,
) -> Vec<Effect> {
    let Some(server) = state.pe.server.as_ref().cloned() else {
        state.messages_error(NO_CHAT);
        return vec![];
    };
    let jobs: Vec<_> = keys
        .iter()
        .filter(|k| state.pe.style_job(k).is_none())
        .filter_map(|k| style_job(state, &server, k, ask))
        .collect();
    if jobs.is_empty() {
        return vec![];
    }
    if state.pe.styles_waiting() == 0 {
        state.pe.style_batch = 0;
    }
    state.pe.style_batch += jobs.len();
    let jobs = jobs
        .into_iter()
        .map(|(j, built, sent)| {
            if let PeKey::Style(k) = &j.key {
                state.pe.style_from.insert(k.clone(), built);
                state.pe.style_failed.remove(k);
                state.pe.style_tries.insert(k.clone(), StyleTry { ask: ask.clone(), sent, retry });
            }
            queue_job(state, j)
        })
        .collect();
    vec![Effect::PeAsk { servers: pool(state, server), jobs }]
}

/// The request improving a style's prompt, with the prompt built for it
/// now; None if the style won't run.
fn style_job(
    state: &AppState,
    server: &increart_core::pe::PeServer,
    key: &increart_core::prompts::PresetKey,
    settings: &EnhanceAsk,
) -> Option<(PeJob, String, String)> {
    use increart_core::pe::{STYLE_PROMPT, StyleAsk, style_message};
    let f = &state.generate;
    let (_, p) = running_styles(state).into_iter().find(|(k, _)| k == key)?;
    let built = built_style_prompt(state, key, &p.prompt);
    let prompt = if settings.from_shown { f.sweep.prompt_for(&f.prompt, key, &p.prompt) } else { built.clone() };
    let root = state.library.as_ref()?;
    let (mut images, mut start) = (vec![], false);
    if server.vision() && settings.images {
        images = f.refs().iter().map(|r| r.path(root)).collect();
        if let Some(init) = f.init().filter(|i| !f.refs().contains(i)) {
            images.push(init.path(root));
            start = true;
        }
    }
    let ask = StyleAsk {
        template: f.prompt.clone(),
        style: p.name.clone(),
        style_prompt: p.prompt.clone(),
        prompt: prompt.clone(),
        how: settings.how.clone(),
        images: images.len() - start as usize,
        start,
    };
    let job = PeJob {
        id: 0,
        key: PeKey::Style(key.clone()),
        system: state.config.pe.system_prompt(STYLE_PROMPT).to_string(),
        sampling: settings.sampling.unwrap_or_else(|| state.config.pe.sampling(STYLE_PROMPT)),
        text: style_message(&ask),
        images,
        // as for blanks: small enough to read quickly, the same for every
        // style so they're read once
        image_size: increart_core::pe::BLANK_IMAGE_SIZE,
        max_tokens: settings.max_tokens,
    };
    Some((job, built, prompt))
}

/// The message a Suggest dialog's ask for a blank would send.
pub fn suggest_message(state: &AppState, key: &BlankKey, form: &SuggestForm) -> Option<String> {
    blank_job(state, state.pe.server.as_ref()?, key, Some(form)).map(|j| j.text)
}

/// The request for values for a blank, if the sweep still has it: as the
/// Suggest… dialog says, else the usual way.
fn blank_job(
    state: &AppState,
    server: &increart_core::pe::PeServer,
    key: &BlankKey,
    d: Option<&SuggestForm>,
) -> Option<PeJob> {
    use increart_core::pe::{BlankAsk, FILL_BLANKS, blank_message};
    use increart_core::prompts::{PresetKey, slot_names, sweep_presets};
    let f = &state.generate;
    let refs = f.refs().len();
    // styles only for a style sweep (a single run's blanks are its prompt's)
    let styled = f.sweep.on && f.sweep.vary == increart_core::prompts::SweepVary::Style;
    let fit: Vec<_> = sweep_presets(&state.prompt_presets, &f.sweep)
        .into_iter()
        .filter(|(_, p)| styled && crate::state::needs_refs(&p.prompt) <= refs)
        .collect();
    let has = |p: &str| slot_names([p]).contains(&key.slot);
    let (styles, filled) = match &key.style {
        None => {
            let styles: Vec<_> =
                fit.iter().filter(|c| f.sweep.blank_texts(&[**c]).into_iter().any(has)).map(|(_, p)| *p).collect();
            if styles.is_empty() && !has(&f.prompt) {
                return None;
            }
            (styles, f.sweep.slots.clone())
        }
        Some(k) => {
            let (_, p) = fit.iter().find(|(t, p)| PresetKey { task: t.name.clone(), name: p.name.clone() } == *k)?;
            (vec![*p], f.sweep.slots_for(k))
        }
    };
    let root = state.library.as_ref()?;
    let (mut images, mut start) = (vec![], false);
    if server.vision() && d.is_none_or(|d| d.images) {
        images = f.refs().iter().map(|r| r.path(root)).collect();
        if let Some(init) = f.init().filter(|i| !f.refs().contains(i)) {
            images.push(init.path(root));
            start = true;
        }
    }
    let offered = if d.is_none_or(|d| d.avoid) { state.blank_offered(key) } else { &[] };
    let ask = BlankAsk {
        slot: key.slot.clone(),
        template: f.prompt.clone(),
        styles: styles.iter().map(|p| (p.name.clone(), p.prompt.clone())).collect(),
        filled,
        images: images.len() - start as usize,
        start,
        count: d.map_or(SUGGEST_COUNT, |d| d.count),
        // the latest, should there be very many
        avoid: offered[offered.len().saturating_sub(MAX_AVOID)..].to_vec(),
        how: d.map_or(String::new(), |d| d.how.clone()),
    };
    Some(PeJob {
        id: 0,
        key: PeKey::Blank(key.clone()),
        system: state.config.pe.system_prompt(FILL_BLANKS).to_string(),
        sampling: d.map_or_else(|| state.config.pe.sampling(FILL_BLANKS), |d| d.sampling),
        text: blank_message(&ask),
        images,
        image_size: increart_core::pe::BLANK_IMAGE_SIZE,
        // room for longer lists
        max_tokens: 300.max(50 * ask.count as u32),
    })
}

/// After servers opened in the background settle: start the engine with the
/// ones that came up (the others were reported as they failed).
/// Stops the servers in use, showing them as stopping until they're gone
/// ([`Msg::ServersStopped`]).
fn stop_servers(state: &mut AppState) -> Effect {
    state.stopping = state.servers.iter().map(server_label).collect();
    state.servers_opened_with = None;
    Effect::StopServers
}

fn start_with_up(state: &mut AppState, progress: &[ServerProgress]) -> Vec<Effect> {
    let up: Vec<bool> = progress.iter().map(|p| *p == ServerProgress::Up).collect();
    state.servers = state.servers.iter().zip(&up).filter(|(_, u)| **u).map(|(s, _)| s.clone()).collect();
    if state.servers.is_empty() {
        state.messages_error("No server came up: open Servers… to retry or change the preset");
        return vec![Effect::StopServers];
    }
    state.engine_running = true;
    state.servers.iter_mut().for_each(|s| (s.idle_since, s.worked) = (None, false));
    vec![Effect::StartEngine {
        shared: state.config.programs.sd_server.clone(),
        order: state.config.job_order,
        skip_ram_check: state.config.skip_ram_check,
    }]
}

/// After the Generate form's image list changed: saved soon, and if the
/// `<imageN>` numbers the prompt uses moved, a prompt edit to confirm.
/// An edited copy's title: "Edited", then what it was made from.
fn edited_title(state: &AppState, image: &ImageRef) -> String {
    let from = match image.store {
        increart_core::Store::Input => state.input_title(image).filter(|t| !t.is_empty()).map(str::to_string),
        increart_core::Store::Output => state.image_title(image).map(str::to_string).or_else(|| {
            let link = state.output_link(image)?;
            Some(format!("{} seed {}", state.project_of_link(link)?.name(), link.name.seed))
        }),
    };
    from.map_or("Edited image".into(), |f| format!("Edited: {f}"))
}

/// A saved edited copy: the editor closes and the copy goes where it was
/// asked to.
fn edited(state: &mut AppState, image: ImageRef, to: EditedTo) -> Vec<Effect> {
    state.markup = None;
    let before = state.generate.refs();
    match to {
        EditedTo::Library => {
            state.messages_info("Edited copy saved (Input library → Show edited copies)");
            vec![]
        }
        EditedTo::Generate(project, i, replace) if state.project.as_ref() == Some(&project) => {
            let images = &mut state.generate.images;
            if replace && i < images.len() {
                // the same <imageN>, now the new copy
                let old = std::mem::replace(&mut images[i].image, image.clone());
                let before = before.into_iter().map(|x| if x == old { image.clone() } else { x }).collect();
                return images_changed(state, before);
            } else {
                let at = (i + 1).min(images.len());
                images.insert(at, increart_core::model::GenerateImage { image, reference: true, start: false });
            }
            images_changed(state, before)
        }
        EditedTo::Generate(..) => {
            state.messages_info("Edited copy saved; Generate is on another project now");
            vec![]
        }
        EditedTo::UseInGenerate => {
            let Some(name) = state.project_entry().map(|p| p.name()) else {
                state.messages_info("Edited copy saved; select a project to use it in Generate");
                return vec![];
            };
            state.generate.add_reference(image.clone());
            let tag =
                state.generate.images.iter().position(|x| x.image == image).and_then(|i| state.generate.tag_of(i));
            state.messages_info(format!(
                "Edited copy added to {name}'s Generate form as <image{}>",
                tag.map_or("?".into(), |t| t.to_string())
            ));
            images_changed(state, before)
        }
    }
}

fn images_changed(state: &mut AppState, before: Vec<ImageRef>) -> Vec<Effect> {
    mark_generate(state);
    let after = state.generate.refs();
    let mut map = std::collections::BTreeMap::new();
    for (_, n) in image_tags(&state.generate.prompt) {
        let Some(img) = before.get(n.wrapping_sub(1)) else { continue }; // a tag with no image before either
        let now = after.iter().position(|x| x == img).map(|i| i + 1);
        if now != Some(n) {
            map.insert(n, now);
        }
    }
    if !map.is_empty() {
        let prompt = retag(&state.generate.prompt, &map);
        state.retag = Some(Retag { changes: map.into_iter().collect(), prompt });
    }
    vec![]
}

/// Characters changed between two texts (word diff).
pub fn changed_chars(a: &str, b: &str) -> usize {
    use similar::{ChangeTag, TextDiff};
    TextDiff::from_words(a, b).iter_all_changes().filter(|c| c.tag() != ChangeTag::Equal).map(|c| c.value().len()).sum()
}

/// A history entry for `key` (DESIGN.md, Generate history). A project's
/// entry has the whole Generate form when it's the open project, and the
/// project's newest run.
fn record(state: &mut AppState, key: HistoryKey, kind: HistoryKind, text: String, extra: String) -> Vec<Effect> {
    let mut entry = HistoryEntry::text(Default::default(), kind, text, extra);
    if let HistoryKey::Project(p) = &key {
        if state.project.as_ref() == Some(p) {
            entry.form = Some(Box::new(state.generate.to_draft(&state.config)));
        }
        entry.after = newest_run(state, p, &[]);
    }
    record_entry(state, key, entry)
}

/// Puts a history entry's whole form back in Generate. The label and first
/// seed stay as they are (the next run's); images no longer in the library
/// and a model preset that's gone are left out, with a message.
fn restore_form(state: &mut AppState, d: &increart_core::model::GenerateDraft) {
    let mut f = GenerateForm::from_draft(d, &state.config);
    f.label = state.generate.label.clone();
    f.first_seed = state.generate.first_seed;
    let mut notes = vec![];
    if !state.config.recipes.iter().any(|r| r.name == d.preset) {
        f.preset = state.generate.preset;
        notes.push(format!("model preset {} no longer exists, kept the current one", d.preset));
    }
    if let Some(root) = &state.library {
        let before = f.images.len();
        f.images.retain(|i| i.image.path(root).is_file());
        let gone = before - f.images.len();
        if gone > 0 {
            notes.push(format!("{gone} image(s) no longer in the library were left out"));
        }
    }
    // the values offered since are kept
    f.sweep.suggestions = std::mem::take(&mut state.generate.sweep.suggestions);
    f.sweep.batch_ask = state.generate.sweep.batch_ask.take();
    f.sweep.enhance_ask = state.generate.sweep.enhance_ask.take();
    state.generate = f;
    runnable_preset(state);
    if notes.is_empty() {
        state.messages_info("Restored the prompt and settings");
    } else {
        state.messages_error(format!("Restored; {}", notes.join("; ")));
    }
}

/// Each entry's runs and the run it follows: stored in newer entries; for
/// older ones, the runs labelled as the queued checkpoint says, and the
/// newest run made before the entry.
fn resolve_history_runs(state: &AppState, v: &mut HistoryView) {
    let runs: &[increart_core::model::Run] = match &v.key {
        HistoryKey::Project(p) => state.index.project(p).map_or(&[], |p| p.runs.as_slice()),
        _ => &[],
    };
    v.runs = v
        .entries
        .iter()
        .map(|e| match &e.kind {
            HistoryKind::Queued(_) if !e.runs.is_empty() => e.runs.clone(),
            HistoryKind::Queued(label) => runs
                .iter()
                .filter(|r| &r.label == label || r.sweep.as_ref().is_some_and(|s| &s.name == label))
                .map(|r| r.id.clone())
                .collect(),
            _ => vec![],
        })
        .collect();
    v.after = v
        .entries
        .iter()
        .zip(&v.runs)
        .map(|(e, mine)| {
            if e.form.is_some() {
                return e.after.clone();
            }
            runs.iter()
                .filter(|r| r.created <= e.time && !mine.contains(&r.id))
                .max_by_key(|r| (r.created, r.id.clone()))
                .map(|r| r.id.clone())
        })
        .collect();
}

/// The run before `run` for the history's "led to" filter: what its queued
/// checkpoint followed, else the project's run made just before it.
fn run_before(state: &AppState, v: &HistoryView, run: &Id) -> Option<Id> {
    if let Some(i) = (0..v.entries.len()).find(|i| v.runs_of(*i).contains(run)) {
        return v.after.get(i).cloned().flatten();
    }
    let (p, r) = state.index.run(run)?;
    p.runs.iter().filter(|x| x.created < r.created).max_by_key(|x| (x.created, x.id.clone())).map(|x| x.id.clone())
}

/// The queued checkpoint: the form a run (or a sweep's runs) was made from.
fn record_queued(state: &mut AppState, q: Queued, runs: Vec<Id>) -> Vec<Effect> {
    let after = newest_run(state, &q.project, &runs);
    let entry = HistoryEntry {
        time: Default::default(),
        kind: HistoryKind::Queued(q.form.label.clone()),
        text: q.form.prompt.clone(),
        extra: q.form.negative_prompt.clone(),
        form: Some(q.form),
        runs,
        after,
    };
    record_entry(state, HistoryKey::Project(q.project), entry)
}

/// A project's newest run (by creation), leaving out `except`.
fn newest_run(state: &AppState, project: &Id, except: &[Id]) -> Option<Id> {
    let p = state.index.project(project)?;
    p.runs.iter().filter(|r| !except.contains(&r.id)).max_by_key(|r| (r.created, r.id.clone())).map(|r| r.id.clone())
}

/// Records an entry: the same texts and settings again are skipped; a small
/// edit soon after an edit replaces it (a settings change alone is small);
/// anything else, and every checkpoint, is a new entry.
fn record_entry(state: &mut AppState, key: HistoryKey, entry: HistoryEntry) -> Vec<Effect> {
    if state.library.is_none() || (entry.text.trim().is_empty() && entry.extra.trim().is_empty()) {
        return vec![];
    }
    let edit = entry.kind == HistoryKind::Edit;
    let last = state.history_last.get(&key);
    if last.is_some_and(|l| l.entry.same_content(&entry) && (edit || l.entry.kind == entry.kind)) {
        return vec![];
    }
    let replace_last = edit
        && last.is_some_and(|l| {
            let (t, x) = (&l.entry.text, &l.entry.extra);
            l.entry.kind == HistoryKind::Edit
                && state.now - l.at < HISTORY_MERGE_SECS
                && changed_chars(&format!("{t}\n{x}"), &format!("{}\n{}", entry.text, entry.extra))
                    < HISTORY_MERGE_CHARS
        });
    let at = if replace_last { last.map_or(state.now, |l| l.at) } else { state.now };
    state.history_last.insert(key.clone(), LastEntry { at, entry: entry.clone() });
    vec![Effect::RecordHistory { key, entry: Box::new(entry), replace_last }]
}

/// Records the Edit idea text after a pause.
fn flush_idea_edit(state: &mut AppState) -> Vec<Effect> {
    if state.idea_edit_dirty.take().is_none() {
        return vec![];
    }
    let Some(ed) = state.idea_edit.clone() else { return vec![] };
    record(state, HistoryKey::Idea(ed.id), HistoryKind::Edit, ed.text, ed.title)
}

/// After the library changed: list the folder view's folder again if it
/// is showing.
fn relist(state: &mut AppState) -> Vec<Effect> {
    let showing = match state.tab {
        Tab::Projects => state.projects_tab == ProjectsTab::Folder,
        Tab::OutputLibrary => state.output_tab == OutputTab::Folder,
        Tab::InputLibrary => state.lib_tab == LibTab::Folder,
        Tab::IdeaInbox => state.inbox_tab == InboxTab::Folder,
        _ => false,
    };
    let fv = &mut state.folder_view;
    if showing && fv.listed.is_none() {
        let path = fv.path.clone();
        fv.listed = Some(path.clone());
        return fv_list(state, path);
    }
    vec![]
}

/// Lists the folder view again when its listing is out of date.
fn relist_if_stale(state: &mut AppState) -> Vec<Effect> {
    if state.folder_view.listed.as_ref() != Some(&state.folder_view.path) {
        state.folder_view.listed = None;
    }
    relist(state)
}

/// Makes `folder_view` the view of the tab showing, parking the one that
/// was.
fn fv_sync(state: &mut AppState) {
    let place = match state.tab {
        Tab::IdeaInbox => FvPlace::Inbox,
        Tab::InputLibrary => FvPlace::InputLibrary,
        Tab::OutputLibrary => FvPlace::Collections,
        _ => FvPlace::Projects,
    };
    fv_activate(state, place);
}

fn fv_activate(state: &mut AppState, place: FvPlace) {
    if state.folder_view.place != place
        && let Some(i) = state.parked_folder_views.iter().position(|v| v.place == place)
    {
        std::mem::swap(&mut state.folder_view, &mut state.parked_folder_views[i]);
    }
}

/// The place a path is in: the view with the longest root it's under
/// (`input/library/…` is the library's, not the inbox's).
fn place_of(path: &std::path::Path) -> Option<FvPlace> {
    FvPlace::ALL.into_iter().filter(|p| path.starts_with(p.root())).max_by_key(|p| p.root().len())
}

/// The folder view a path belongs to (a listing may arrive after a tab
/// switch).
fn fv_for<'a>(state: &'a mut AppState, path: &std::path::Path) -> &'a mut FolderView {
    match place_of(path) {
        Some(p) if p != state.folder_view.place => match state.parked_folder_views.iter_mut().find(|v| v.place == p) {
            Some(v) => v,
            None => &mut state.folder_view,
        },
        _ => &mut state.folder_view,
    }
}

/// Lists a folder: the virtual smart folders from the index, the rest from
/// disk.
fn fv_list(state: &mut AppState, path: PathBuf) -> Vec<Effect> {
    if is_smart(&path) {
        let rel = path.strip_prefix(SMART_DIR).map(PathBuf::from).unwrap_or_default();
        let entries = state.smart_listing(&rel);
        return update(state, Msg::FvListed(path, entries));
    }
    vec![Effect::ListDir(path)]
}

/// Where an Output library source is in its folder view.
fn source_path(state: &AppState, source: &OutputSource) -> PathBuf {
    let smart = PathBuf::from(SMART_DIR);
    match source {
        OutputSource::AllProjects => PathBuf::from("collections"),
        OutputSource::Collection(f) => PathBuf::from("collections").join(f),
        OutputSource::Smart(Smart::Day(d)) => smart.join("by date").join(d),
        OutputSource::Smart(Smart::Transparent) => smart.join("transparent"),
        OutputSource::ProjectFolder(f) => smart.join("by project").join(f),
        OutputSource::Run(id) => match state.index.run(id) {
            Some((p, _)) => source_path(state, &OutputSource::Project(p.project.id.clone())),
            None => smart.join("by project"),
        },
        OutputSource::Project(id) => {
            let dir = state.index.projects.iter().find(|p| &p.project.id == id).map(|p| &p.dir);
            let rel = dir.and_then(|d| d.strip_prefix("projects").ok());
            rel.map_or_else(|| smart.join("by project"), |r| smart.join("by project").join(r))
        }
        OutputSource::Smart(Smart::Idea(_)) => {
            let group = "by source idea";
            let names = state.smart_children(std::path::Path::new(group)).unwrap_or_default();
            let name = names
                .into_iter()
                .find(|n| state.smart_source(&std::path::Path::new(group).join(n)).as_ref() == Some(source));
            name.map_or_else(|| smart.join(group), |n| smart.join(group).join(n))
        }
    }
}

/// Navigates the folder view (within its root), keeping the selected project
/// in sync when it enters one.
fn fv_go(state: &mut AppState, path: PathBuf, push_history: bool) -> Vec<Effect> {
    let path = PathBuf::from(path.to_string_lossy().trim_end_matches('/'));
    if let Some(p) = place_of(&path) {
        fv_activate(state, p);
    }
    if !path.starts_with(&state.folder_view.root) {
        let root = state.folder_view.root.display().to_string();
        state.messages_error(format!("{} is outside {root}", path.display()));
        return vec![];
    }
    let fv = &mut state.folder_view;
    if push_history && path != fv.path {
        fv.back.push(fv.path.clone());
        fv.forward.clear();
    }
    fv.path = path.clone();
    fv.selected.clear();
    fv.anchor = None;
    fv.path_edit = None;
    fv.listed = Some(path.clone());
    let mut fx = fv_list(state, path.clone());
    let project = state.index.projects.iter().find(|p| path.starts_with(&p.dir)).map(|p| p.project.id.clone());
    if let Some(id) = project
        && state.project.as_ref() != Some(&id)
    {
        fx.extend(update(state, Msg::SelectProject(id)));
    }
    fx
}

/// An output image where it is: unrated in Review, else selected in the
/// Output library Grid under its project.
fn go_to_output(state: &mut AppState, link: increart_core::scan::OutputLink) -> Vec<Effect> {
    let project = state.project_of_link(&link).map(|p| p.project.id.clone());
    match link.verdict {
        None => {
            leave_sweep(state);
            state.review_folder = None;
            state.review_scope = project;
            state.review_view = ReviewView::Single;
            state.review_cursor = state.review_queue().iter().position(|o| o.path == link.path).unwrap_or(0);
            unfollow(state);
            state.tab = Tab::Review;
            vec![]
        }
        Some(v) => {
            state.tab = Tab::OutputLibrary;
            state.output_tab = OutputTab::Grid;
            let preset = if v == Verdict::Bad { OutputPreset::Rejected } else { OutputPreset::Kept };
            if !state.output_filter.ratings.allows(Some(v)) {
                state.output_preset = preset;
                state.output_filter = preset.filter();
            }
            if let Some(p) = project {
                state.output_source = OutputSource::Project(p);
            }
            update(state, Msg::OutputSelect(Some(link.image.clone())))
        }
    }
}

/// Enter / double-click on a folder-view entry.
fn fv_open(state: &mut AppState, name: &str) -> Vec<Effect> {
    let fv = &state.folder_view;
    let Some(e) = fv.entries.iter().find(|e| e.name == name).cloned() else { return vec![] };
    let path = fv.path.join(name);
    match e.kind {
        k if k.is_dir() => fv_go(state, path, true),
        // a collection's or smart folder's image: the Output library Grid,
        // showing that collection / smart folder, with it selected
        // an idea's image: its Edit idea window
        EntryKind::Image(_) if matches!(state.folder_view.place, FvPlace::InputLibrary | FvPlace::Inbox) => {
            match state.folder_view.path.file_name().and_then(|n| n.to_string_lossy().parse::<Id>().ok()) {
                Some(id) => update(state, Msg::OpenIdea(id)),
                None => vec![],
            }
        }
        EntryKind::Image(img) if state.folder_view.place == FvPlace::Collections => {
            let dir = state.folder_view.path.clone();
            let source = if is_smart(&dir) {
                state.smart_source(dir.strip_prefix(SMART_DIR).unwrap_or(&dir))
            } else {
                dir.strip_prefix("collections").ok().map(|f| OutputSource::Collection(f.to_path_buf()))
            };
            state.output_tab = OutputTab::Grid;
            fv_sync(state);
            if let Some(s) = source {
                state.output_source = s;
            }
            update(state, Msg::OutputSelect(Some(img)))
        }
        EntryKind::Image(_) => {
            let Some(link) = state.index.projects.iter().flat_map(|p| &p.outputs).find(|o| o.path == path).cloned()
            else {
                return vec![];
            };
            go_to_output(state, link)
        }
        _ => update(state, Msg::ShowInFileManager(path)),
    }
}

/// Paste the clipboard into the current folder (or a subfolder of it).
fn fv_paste(state: &mut AppState, sub: Option<String>) -> Vec<Effect> {
    let Some(clip) = state.fv_clipboard.clone() else { return vec![] };
    let dest = match &sub {
        Some(d) => state.folder_view.path.join(d),
        None => state.folder_view.path.clone(),
    };
    let fx = transfer(state, &clip.paths, clip.cut, &dest);
    if clip.cut {
        state.fv_clipboard = None;
    }
    fx
}

/// Moves (`cut`) or copies folders, projects and image links (paths relative
/// to the library root) into `dest`, for paste and drag and drop.
fn transfer(state: &mut AppState, paths: &[PathBuf], cut: bool, dest: &std::path::Path) -> Vec<Effect> {
    let dest = dest.to_path_buf();
    let mut fx = vec![];
    for src in paths {
        if (cut && src.parent() == Some(dest.as_path())) || *src == dest {
            continue; // moving to where it already is, or dropped on itself
        }
        if is_smart(&dest) {
            state.messages_error("Smart folders are read-only");
            continue;
        }
        if let Ok(to) = dest.strip_prefix("input/library") {
            let Ok(from) = src.strip_prefix("input/library") else {
                state.messages_error("Only folders and ideas of the input library can go here");
                continue;
            };
            if dest.starts_with(src) {
                state.messages_error(format!("Can't put {} inside itself", src.display()));
                continue;
            }
            if state.index.ideas.iter().any(|i| dest.starts_with(&i.dir)) {
                state.messages_error("Folders and ideas can't go inside an idea");
                continue;
            }
            let (from, to) = (from.to_path_buf(), to.to_path_buf());
            let t = Tree::InputLibrary;
            fx.extend(lib(state, if cut { LibOp::MoveNode(t, from, to) } else { LibOp::CopyNode(t, from, to) }));
            continue;
        }
        if dest.starts_with("input") {
            state.messages_error("The inbox takes new ideas from inbox_drop/ (or the New idea form)");
            continue;
        }
        if let Ok(to) = dest.strip_prefix("collections") {
            let is_image = StoreFormat::of_path(src).is_some();
            if is_smart(src) || (is_image && !src.starts_with("collections")) {
                // an image from a smart folder or a project: added to the collection
                match state.image_at(src) {
                    Some(img) => fx.extend(lib(state, LibOp::AddToCollection(to.to_path_buf(), vec![img]))),
                    None => state.messages_error(format!("{} is not an image", src.display())),
                }
                continue;
            }
            let Ok(from) = src.strip_prefix("collections") else {
                state.messages_error("Only images can go into collections");
                continue;
            };
            if dest.starts_with(src) {
                state.messages_error(format!("Can't put {} inside itself", src.display()));
                continue;
            }
            let (from, to) = (from.to_path_buf(), to.to_path_buf());
            let op = if cut {
                LibOp::MoveNode(Tree::Collections, from, to)
            } else {
                LibOp::CopyNode(Tree::Collections, from, to)
            };
            fx.extend(lib(state, op));
            continue;
        }
        if dest.starts_with(src) {
            state.messages_error(format!("Can't put {} inside itself", src.display()));
            continue;
        }
        if state.index.projects.iter().any(|p| dest.starts_with(&p.dir)) && StoreFormat::of_path(src).is_none() {
            state.messages_error("Folders and projects can't go inside a project");
            continue;
        }
        if StoreFormat::of_path(src).is_some() {
            // An image link: only between the review folders of its own project.
            let project_of = |p: &std::path::Path| {
                state.index.projects.iter().find(|x| p.starts_with(&x.dir)).map(|x| x.dir.clone())
            };
            let verdict = match dest
                .strip_prefix(project_of(&dest).unwrap_or_default())
                .ok()
                .map(|r| r.to_string_lossy().into_owned())
            {
                Some(r) if r == "unreviewed" => Some(None),
                Some(r) if r == "reviewed/good" => Some(Some(Verdict::Good)),
                Some(r) if r == "reviewed/neutral" => Some(Some(Verdict::Neutral)),
                Some(r) if r == "reviewed/bad" => Some(Some(Verdict::Bad)),
                _ => None,
            };
            match (verdict, project_of(src) == project_of(&dest)) {
                (Some(v), true) => fx.extend(update(state, Msg::ReviewLink(src.clone(), v))),
                _ => state.messages_error(
                    "Image links can only be moved between unreviewed/ and reviewed/*/ of their own project (use → input ref for others)",
                ),
            }
            continue;
        }
        let (Ok(from), Ok(to)) = (src.strip_prefix("projects"), dest.strip_prefix("projects")) else { continue };
        let op = if cut {
            LibOp::MoveNode(Tree::Projects, from.to_path_buf(), to.to_path_buf())
        } else {
            LibOp::CopyNode(Tree::Projects, from.to_path_buf(), to.to_path_buf())
        };
        fx.extend(lib(state, op));
    }
    fx
}

/// Once no style is waiting, any blanks prompt enhance added are asked
/// about.
fn ask_added_blanks(state: &mut AppState) {
    if state.pe.styles_waiting() == 0 && !state.pe.added_blanks.is_empty() {
        state.pe.added_blanks_ask = true;
    }
}

fn mark_generate(state: &mut AppState) {
    if state.project.is_some() {
        state.generate_dirty = Some(state.now);
    }
}

/// Saves the selected project's Generate draft now, if it was edited.
fn flush_generate(state: &mut AppState) -> Vec<Effect> {
    match (state.generate_dirty.take(), state.project.clone()) {
        (Some(_), Some(p)) => {
            let d = state.generate.to_draft(&state.config);
            if let Some(e) = state.index.projects.iter_mut().find(|e| e.project.id == p) {
                e.draft = Some(d.clone());
            }
            state.saving += 1;
            let mut fx = vec![Effect::SaveDraft(DraftSave::Generate(p.clone(), Box::new(d)))];
            let (t, x) = (state.generate.prompt.clone(), state.generate.negative.clone());
            fx.extend(record(state, HistoryKey::Project(p), HistoryKind::Edit, t, x));
            fx
        }
        _ => vec![],
    }
}

/// Prompt enhance's asks for the Generate form (its sweep's blanks and
/// styles' prompts), queued or answering.
fn generate_asks(state: &AppState) -> Vec<u64> {
    state.pe.jobs.iter().filter(|j| matches!(j.key, PeKey::Blank(_) | PeKey::Style(_))).map(|j| j.id).collect()
}

/// Asks before `then` leaves the project's Generate form while prompt
/// enhance is still answering for it (leaving stops those asks): false when
/// there's nothing to ask.
fn ask_leaving_generate(state: &mut AppState, then: &Msg) -> bool {
    let n = generate_asks(state).len();
    if n == 0 {
        return false;
    }
    let form = state.project_entry().map_or("the Generate form".into(), |p| format!("{}'s Generate form", p.name()));
    let text = format!(
        "Prompt enhance is still working on {form}: {n} ask(s) for blank values or style prompts \
         are waiting. Leaving the project stops them (what it already answered is kept)."
    );
    state.confirm = Some(Confirm::new(text, "Leave", Msg::LeaveGenerate(Box::new(then.clone()))));
    true
}

/// Leaving the project's Generate form: saved, and the asks
/// for its sweep's blanks stopped (their values are its own).
fn leave_generate(state: &mut AppState) -> Vec<Effect> {
    let mut fx = flush_generate(state);
    let ids = generate_asks(state);
    if !ids.is_empty() {
        fx.extend(update(state, Msg::PeStop(ids)));
    }
    state.pe.suggest = None;
    state.pe.batch_suggest = None;
    state.pe.blank_failed.clear();
    state.pe.style_failed.clear();
    state.pe.style_dialog = None;
    // blanks added and not asked about yet stay (the table shows them)
    state.pe.added_blanks.clear();
    state.pe.added_blanks_ask = false;
    fx
}

/// Saves the New idea draft now, if it was edited (an empty one is removed).
fn flush_idea(state: &mut AppState) -> Vec<Effect> {
    if state.idea_dirty.take().is_none() {
        return vec![];
    }
    let save = if state.draft.is_empty() && state.draft.tags.trim().is_empty() {
        DraftSave::ClearIdea
    } else {
        DraftSave::Idea(state.draft.to_file())
    };
    state.saving += 1;
    let mut fx = vec![Effect::SaveDraft(save)];
    let (t, x) = (state.draft.text.clone(), state.draft.title.clone());
    fx.extend(record(state, HistoryKey::NewIdea, HistoryKind::Edit, t, x));
    fx
}

/// The Generate form for the newly selected project: its saved draft, else a
/// fresh form. The label moves on if a run already has it; the first seed
/// continues after the project's seeds, unless the draft chose seed 0.
fn load_generate(state: &mut AppState) {
    let draft = state.project_entry().and_then(|p| p.draft.clone());
    let latest = state.project_entry().and_then(|p| state.latest_run(p)).cloned();
    let mut f = match (&draft, &latest) {
        (Some(d), _) => GenerateForm::from_draft(d, &state.config),
        // No draft (e.g. runs made before drafts existed): carry on from the
        // latest run's prompt, refs and settings.
        (None, Some(run)) => GenerateForm::from_run(run, &state.config),
        (None, None) => GenerateForm::new(&state.config),
    };
    let used = state.project_entry().is_some_and(|p| p.runs.iter().any(|r| r.label == f.label));
    if f.label.is_empty() || used {
        f.label = next_label(state);
    }
    if draft.is_none() || f.first_seed != 0 {
        f.first_seed = f.first_seed.max(state.next_seed());
    }
    state.generate = f;
    state.generate_dirty = None;
    runnable_preset(state);
}

/// The form's model preset swapped for one an open server runs, when its
/// own can't run anywhere and one with the same recipe can (a draft or run
/// made for sd-server's int8 files, opened with KoboldCpp); said.
fn runnable_preset(state: &mut AppState) {
    let f = &state.generate;
    if f.sweep.on && f.sweep.vary == increart_core::prompts::SweepVary::Model {
        return;
    }
    let Some(j) = state.runnable_alternative(f.preset) else { return };
    let (from, to) = (state.config.recipes[f.preset].name.clone(), state.config.recipes[j].name.clone());
    state.generate.preset = j;
    state.messages_info(format!("Model preset {to} instead of {from}, which no open server can run"));
}

/// A queued run or sweep is saved: Queue works for its label again.
fn saved_queued(state: &mut AppState, q: &Queued) {
    state.queueing.retain(|(p, l)| !(*p == q.project && *l == q.form.label));
}

fn op_done(state: &mut AppState, ok: OpOk) -> Vec<Effect> {
    match ok {
        OpOk::Done | OpOk::Idea(_) => {}
        OpOk::Tiled(_) => state.messages_info("Tiled image added to the input library"),
        OpOk::Edited(id, to) => return edited(state, ImageRef::input(id), to),
        OpOk::ImportedRefs(ids) => {
            return update(state, Msg::AddRefs(ids.into_iter().map(ImageRef::input).collect()));
        }
        OpOk::Imported(ids) => {
            for id in ids {
                if !state.draft.images.contains(&id) {
                    state.draft.images.push(id);
                }
            }
            state.idea_dirty = Some(state.now);
        }
        OpOk::ProjectFromIdeas { project, prompt, refs, idea } => {
            let fx = leave_generate(state);
            state.project = Some(project);
            state.proj_folder = None;
            state.tab = Tab::Projects;
            state.projects_tab = ProjectsTab::Generate;
            let mut f = GenerateForm::new(&state.config);
            f.label = "r00".into();
            f.prompt = prompt;
            f.images = images_of(&refs, None);
            f.source_idea = idea;
            state.generate = f;
            mark_generate(state);
            return fx;
        }
        OpOk::Created(tree, path) => {
            let mut fx = vec![];
            if tree == Tree::Projects {
                let dir = std::path::Path::new("projects").join(&path);
                if let Some(id) = state.index.projects.iter().find(|p| p.dir == dir).map(|p| p.project.id.clone()) {
                    fx = leave_generate(state);
                    state.project = Some(id);
                    state.proj_folder = None;
                    load_generate(state);
                    // a project is named before it is made
                    return fx;
                }
            }
            let name = path.file_name().unwrap_or_default().to_string_lossy().into();
            state.renaming = Some(Renaming { tree, name, path, creating: false });
            return fx;
        }
        OpOk::Duplicated(path) => {
            let dir = std::path::Path::new("projects").join(&path);
            let Some(id) = state.index.projects.iter().find(|p| p.dir == dir).map(|p| p.project.id.clone()) else {
                return vec![];
            };
            let fx = leave_generate(state);
            state.project = Some(id);
            state.proj_folder = None;
            load_generate(state);
            return fx;
        }
        OpOk::ProjectReady { path, form, then } => {
            let dir = std::path::Path::new("projects").join(&path);
            let Some(id) = state.index.projects.iter().find(|p| p.dir == dir).map(|p| p.project.id.clone()) else {
                return vec![];
            };
            let fx = leave_generate(state);
            state.project = Some(id);
            state.proj_folder = None;
            let mut f = form;
            f.label = next_label(state);
            f.first_seed = state.next_seed();
            state.generate = *f;
            mark_generate(state);
            state.tab = Tab::Projects;
            state.projects_tab = ProjectsTab::Generate;
            // named in the dialog already
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            state.messages_info(format!("Project {name} ready"));
            let pick = match then {
                StartRefs::None => vec![],
                StartRefs::Library => update(state, Msg::RefPicker(true)),
                StartRefs::Files => update(state, Msg::Pick(PickKind::GenerateRefs)),
            };
            return fx.into_iter().chain(pick).collect();
        }
        OpOk::SeedsAdded(run, n) => {
            let label = state.index.run(&run).map(|(p, r)| format!("{} · {}", p.name(), r.label)).unwrap_or_default();
            if state.engine_running {
                state.active_runs.insert(run.clone());
                state.messages_info(format!("+{n} seeds queued for {label}"));
                return vec![Effect::QueueRun(run)];
            }
            state.messages_info(format!("+{n} seeds added to {label}; they run once servers are connected"));
        }
        OpOk::Sweep(ids, name, queued) => {
            saved_queued(state, &queued);
            if state.engine_running {
                state.active_runs.extend(ids.iter().cloned());
            }
            let fx = record_queued(state, *queued, ids.clone());
            state.generate.label = next_label(state);
            state.generate.first_seed = state.next_seed();
            mark_generate(state);
            state.messages_info(format!("Sweep {name} queued: {} runs", ids.len()));
            return fx;
        }
        OpOk::Run(id, queued) => {
            saved_queued(state, &queued);
            if state.engine_running {
                state.active_runs.insert(id.clone());
            }
            let fx = record_queued(state, *queued, vec![id]);
            state.generate.label = next_label(state);
            state.generate.first_seed = state.next_seed();
            mark_generate(state);
            state.messages_info("Run queued");
            return fx;
        }
        OpOk::Reviewed { done, undo, error } => {
            if !undo && !done.is_empty() {
                state.review_undo.push(done);
            }
            if let Some(e) = error {
                state.messages_error(e);
            }
        }
    }
    vec![]
}

/// The lines of a generation log the Job window holds.
pub const JOB_LOG_LINES: usize = 5000;

fn engine_event(state: &mut AppState, e: Event) -> Vec<Effect> {
    match e {
        Event::Server { server, state: s, model, idle, runs } => {
            state.server_jobs.insert(server, runs.clone());
            state.alerts.server_runs.entry(server).or_default().extend(runs);
            if let Some(st) = state.servers.get_mut(server) {
                st.model = model.map(|m| m.recipe);
                let was_idle = st.idle_since.is_some();
                st.idle_since = if idle { st.idle_since.or(Some(state.now)) } else { None };
                if s == "ready" && !idle {
                    st.worked = true;
                    state.alerts.cleared_text = None;
                } else if idle && !was_idle && std::mem::take(&mut st.worked) {
                    // not when the user stopped all it was doing (Stop now)
                    let runs = state.alerts.server_runs.remove(&server).unwrap_or_default();
                    let by_user = !runs.is_empty() && runs.is_subset(&state.alerts.stopped_now);
                    if !by_user {
                        state.alerts.cleared.push(server);
                    }
                } else if s == "connecting" || s.starts_with("unreachable") {
                    st.worked = false;
                    state.alerts.server_runs.remove(&server);
                }
                st.state = s;
            }
            vec![]
        }
        Event::Progress { run, seed, step, total, .. } => {
            let now = state.now;
            state
                .sampling_times
                .entry((run.clone(), seed))
                .and_modify(|t| t.latest = (now, step))
                .or_insert_with(|| crate::timing::Sampling::new(now, step));
            state.progress.insert((run, seed), (step, total));
            vec![]
        }
        Event::Output { run, seed, server, elapsed_s, step_s, .. } => {
            let steps = state.index.run(&run).map_or(0, |(_, r)| r.sample.steps);
            if let (Some(s), true) = (state.servers.get(server), elapsed_s > 0.0) {
                let recent = state.server_speeds.entry(s.name.clone()).or_default();
                recent.push_back(crate::timing::Recent { elapsed_s, step_s, steps });
                while recent.len() > crate::timing::RECENT {
                    recent.pop_front();
                }
            }
            state.sampling_times.remove(&(run.clone(), seed));
            state.progress.remove(&(run, seed));
            vec![Effect::Rescan]
        }
        Event::SeedFailed { run, seed, error, .. } => {
            state.sampling_times.remove(&(run.clone(), seed));
            state.progress.remove(&(run.clone(), seed));
            state.messages_error(format!("run {run} seed {seed}: {error}"));
            vec![]
        }
        Event::RunDone { run } => {
            state.active_runs.remove(&run);
            state.finishing.remove(&run);
            state.progress.retain(|(r, _), _| r != &run);
            state.sampling_times.retain(|(r, _), _| r != &run);
            vec![Effect::Rescan]
        }
        Event::Problem(p) => {
            state.messages_server_error(p);
            vec![]
        }
        Event::Log { run, line } => {
            if state.job_info.as_ref() == Some(&run) {
                state.job_log.push(line);
                let over = state.job_log.len().saturating_sub(JOB_LOG_LINES);
                state.job_log.drain(..over);
            }
            vec![]
        }
    }
}

/// The notification alerts (DESIGN.md, Notifications), checked once a
/// second: each sounds when its condition first passes its threshold and
/// then every `repeat_minutes` while it holds, unless snoozed, muted or (if
/// so set) the window has the focus; a sound held back plays once that ends.
fn check_alerts(state: &mut AppState) -> Vec<Effect> {
    let n = state.config.notifications.clone();
    let now = state.now;
    let mut fx = vec![];

    // a GPU (server) ready with no job; prompt enhance asking counts as work
    if !state.pe.jobs.is_empty() {
        state.alerts.pe_busy_at = Some(now);
    }
    let pe_busy = state.alerts.pe_busy_at.filter(|_| n.pe_counts_as_work);
    let limit = n.gpu_idle.threshold as f64 * 60.0;
    let idle: Vec<(usize, f64)> = state
        .servers
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.idle_since.map(|t| (i, now - pe_busy.map_or(t, |p| t.max(p)))))
        .filter(|(_, d)| *d >= limit)
        .collect();
    let holds = state.engine_running
        && !state.servers.is_empty()
        && if n.all_gpus { idle.len() == state.servers.len() } else { !idle.is_empty() };
    let text = holds.then(|| {
        let mins = |d: f64| (d / 60.0).floor() as u64;
        if n.all_gpus {
            let least = idle.iter().map(|(_, d)| *d).fold(f64::INFINITY, f64::min);
            format!("All GPUs idle for {} min", mins(least))
        } else {
            let (i, d) = idle.iter().copied().fold((0, 0.0), |a, b| if b.1 > a.1 { b } else { a });
            let s = &state.servers[i];
            let name = if s.device.is_empty() { &s.name } else { &s.device };
            format!("{name} idle for {} min", mins(d))
        }
    });
    fx.extend(alert(state, true, n.gpu_idle.on.then_some(text).flatten()));

    // a GPU just finished its last job: once, at once
    let cleared = std::mem::take(&mut state.alerts.cleared);
    if n.queue_cleared.on && state.engine_running && !cleared.is_empty() {
        let text = if n.queue_cleared_all {
            state.servers.iter().all(|s| !s.worked).then(|| "All GPUs finished their queues".to_string())
        } else {
            let names: Vec<&str> = cleared
                .iter()
                .filter_map(|&i| state.servers.get(i))
                .map(|s| if s.device.is_empty() { s.name.as_str() } else { s.device.as_str() })
                .collect();
            let its = if names.len() == 1 { "its queue" } else { "their queues" };
            Some(format!("{} finished {its}", names.join(", ")))
        };
        if let Some(text) = text {
            state.alerts.cleared_text = Some(text.clone());
            if !state.alerts.held(now, n.only_unfocused) {
                fx.extend(ring(state, n.queue_cleared.sound.clone(), text));
            }
        }
    }

    // the review queue
    let waiting = if state.library.is_some() { state.unreviewed_outputs() } else { 0 };
    let text = (waiting > n.review_backlog.threshold as usize).then(|| format!("{waiting} images waiting for review"));
    fx.extend(alert(state, false, n.review_backlog.on.then_some(text).flatten()));
    fx
}

/// One alert's step: `text` is what holds (`None`: nothing, which resets it).
fn alert(state: &mut AppState, idle: bool, text: Option<String>) -> Vec<Effect> {
    let n = &state.config.notifications;
    let rule = if idle { n.gpu_idle.clone() } else { n.review_backlog.clone() };
    let held = state.alerts.held(state.now, n.only_unfocused);
    let a = &mut state.alerts;
    let (played, shown) =
        if idle { (&mut a.idle_played, &mut a.idle_text) } else { (&mut a.backlog_played, &mut a.backlog_text) };
    *shown = text.clone();
    let Some(text) = text else {
        *played = None;
        return vec![];
    };
    let due = played.is_none_or(|t| state.now - t >= rule.repeat_minutes.max(1) as f64 * 60.0);
    if !due || held {
        return vec![];
    }
    *played = Some(state.now);
    ring(state, rule.sound, text)
}

/// Sound an alert now, with the taskbar flash and message if so set.
fn ring(state: &mut AppState, sound: increart_core::notify::Sound, text: String) -> Vec<Effect> {
    let n = &state.config.notifications;
    if n.flash_taskbar {
        state.alerts.attention = true;
    }
    let fx = vec![Effect::PlaySound { sound, volume: n.volume, player: n.player.clone(), test: false }];
    if n.show_message {
        state.messages_alert(text);
    }
    fx
}
