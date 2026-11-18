package com.example.demo

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map

/** A pinnable note. */
@Serializable
data class Note(
    val id: Int,
    val title: String,
    val pinned: Boolean = false,
    val tags: List<String> = emptyList(),
) {
    val label: String
        get() = if (pinned) "★ $title" else "${title.take(MAX_TITLE)} (${tags.size})"

    companion object {
        const val MAX_TITLE = 40
    }
}

sealed interface Event {
    data object Loaded : Event
    class Failed(val cause: Throwable?) : Event
}

// Titles of the pinned notes, in order.
fun List<Note>.pinnedTitles(): List<String> = filter { it.pinned }.map { it.title }

fun Flow<Note>.labels(): Flow<String> = map { note -> note.label.uppercase() }

fun main() {
    val notes = listOf(Note(1, "Courses", pinned = true), Note(2, "Idées", tags = listOf("vrac")))
    for ((index, note) in notes.withIndex()) {
        when {
            note.pinned -> println("$index : ${note.title}")
            else -> continue
        }
    }
    println(notes.pinnedTitles())
}
