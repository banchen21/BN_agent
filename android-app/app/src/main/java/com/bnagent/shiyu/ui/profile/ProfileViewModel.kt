package com.bnagent.shiyu.ui.profile

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import com.bnagent.shiyu.data.repository.ConversationRepository
import com.bnagent.shiyu.domain.model.Conversation
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.stateIn

/** 个人资料页状态：按会话 id 订阅会话资料。 */
class ProfileViewModel(
    repository: ConversationRepository,
    conversationId: String,
) : ViewModel() {

    val conversation: StateFlow<Conversation?> = repository.conversation(conversationId)
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    companion object {
        fun factory(repository: ConversationRepository, conversationId: String) = viewModelFactory {
            initializer { ProfileViewModel(repository, conversationId) }
        }
    }
}
