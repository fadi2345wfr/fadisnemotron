package de.fadi.nemotron.ui.common

import de.fadi.nemotron.R
import de.fadi.nemotron.data.ApprovalMode

/** The icon on the mode switch above the input field. */
val ApprovalMode.icon: Int
    get() = when (this) {
        ApprovalMode.AskAll -> R.drawable.ic_review
        ApprovalMode.AutoEdits -> R.drawable.ic_edit
        ApprovalMode.AutoAll -> R.drawable.ic_auto
    }

/** The short name on the switch: Prüfen (Request Review), Dateien, Autonom. */
val ApprovalMode.shortLabel: Int
    get() = when (this) {
        ApprovalMode.AskAll -> R.string.mode_review
        ApprovalMode.AutoEdits -> R.string.mode_files
        ApprovalMode.AutoAll -> R.string.mode_auto
    }

/** One sentence on what the mode does. */
val ApprovalMode.hint: Int
    get() = when (this) {
        ApprovalMode.AskAll -> R.string.mode_review_hint
        ApprovalMode.AutoEdits -> R.string.mode_files_hint
        ApprovalMode.AutoAll -> R.string.mode_auto_hint
    }
