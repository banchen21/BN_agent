package com.bnagent.shiyu.ui.contacts

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import com.bnagent.shiyu.data.repository.ConversationRepository
import com.bnagent.shiyu.domain.model.Contact
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn

/**
 * 通讯录页状态：联系人列表 + 常驻搜索。
 *
 * 搜索匹配 `name + role`（大小写不敏感，设计规格 4.4 / 5.2）。
 */
class ContactsViewModel(repository: ConversationRepository) : ViewModel() {

    private val _query = MutableStateFlow("")
    val query: StateFlow<String> = _query.asStateFlow()

    val contacts: StateFlow<List<Contact>> =
        combine(repository.contacts, _query) { contacts, rawQuery ->
            val keyword = rawQuery.trim()
            if (keyword.isEmpty()) {
                contacts
            } else {
                contacts.filter { contact ->
                    contact.name.contains(keyword, ignoreCase = true) ||
                        contact.role.contains(keyword, ignoreCase = true)
                }
            }
        }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), emptyList())

    fun onQueryChange(value: String) {
        _query.value = value
    }

    fun clearQuery() {
        _query.value = ""
    }

    companion object {
        fun factory(repository: ConversationRepository) = viewModelFactory {
            initializer { ContactsViewModel(repository) }
        }
    }
}
