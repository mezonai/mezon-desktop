//! High-performance N-gram Prefix Trie for Vietnamese Autocomplete & Predictive Typing.
//! Zero-allocation reads, instant startup with embedded default corpus transitions.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WordSuggestion {
    pub word: String,
    pub probability: f32,
    pub suggestion_type: String, // "completion" or "next_word"
}

#[derive(Default)]
struct TrieNode {
    children: HashMap<char, TrieNode>,
    is_word: bool,
    frequency: u32,
}

pub struct WordTrie {
    root: TrieNode,
    unigrams: HashMap<String, u32>,
    bigrams: HashMap<String, HashMap<String, u32>>,
    trigrams: HashMap<(String, String), HashMap<String, u32>>,
    total_words: u32,
}

impl WordTrie {
    pub fn new() -> Self {
        let mut trie = Self {
            root: TrieNode::default(),
            unigrams: HashMap::new(),
            bigrams: HashMap::new(),
            trigrams: HashMap::new(),
            total_words: 0,
        };
        trie.train_default_corpus();
        trie
    }

    pub fn insert(&mut self, word: &str, frequency: u32) {
        let clean = word.trim().to_lowercase();
        if clean.is_empty() {
            return;
        }

        let mut curr = &mut self.root;
        for ch in clean.chars() {
            curr = curr.children.entry(ch).or_default();
        }
        curr.is_word = true;
        curr.frequency += frequency;

        *self.unigrams.entry(clean).or_insert(0) += frequency;
        self.total_words += frequency;
    }

    pub fn train_on_corpus(&mut self, sentences: &[&str]) {
        for s in sentences {
            let words: Vec<String> = s
                .split(|c: char| !c.is_alphabetic())
                .filter(|w| !w.is_empty())
                .map(|w| w.to_lowercase())
                .collect();

            if words.is_empty() {
                continue;
            }

            for w in &words {
                self.insert(w, 1);
            }

            // Bigrams
            for i in 0..words.len().saturating_sub(1) {
                let w1 = words[i].clone();
                let w2 = words[i + 1].clone();
                *self.bigrams.entry(w1).or_default().entry(w2).or_insert(0) += 1;
            }

            // Trigrams
            for i in 0..words.len().saturating_sub(2) {
                let key = (words[i].clone(), words[i + 1].clone());
                let w3 = words[i + 2].clone();
                *self.trigrams.entry(key).or_default().entry(w3).or_insert(0) += 1;
            }
        }
    }

    /// Completes a partially typed word prefix (e.g. "mu" -> ["muốn", "mua"]).
    pub fn complete_prefix(&self, prefix: &str, top_k: usize) -> Vec<WordSuggestion> {
        let clean = prefix.trim().to_lowercase();
        if clean.is_empty() {
            return Vec::new();
        }

        let mut curr = &self.root;
        for ch in clean.chars() {
            match curr.children.get(&ch) {
                Some(next) => curr = next,
                None => return Vec::new(),
            }
        }

        let mut matches = Vec::new();
        let mut path: Vec<char> = clean.chars().collect();

        fn dfs(node: &TrieNode, path: &mut Vec<char>, matches: &mut Vec<(String, u32)>) {
            if node.is_word {
                matches.push((path.iter().collect(), node.frequency));
            }
            for (&ch, child) in &node.children {
                path.push(ch);
                dfs(child, path, matches);
                path.pop();
            }
        }

        dfs(curr, &mut path, &mut matches);
        matches.sort_by(|a, b| b.1.cmp(&a.1));

        let total_freq: u32 = matches.iter().map(|(_, f)| *f).sum();
        let denom = if total_freq > 0 {
            total_freq as f32
        } else {
            1.0
        };

        matches
            .into_iter()
            .take(top_k)
            .map(|(word, freq)| WordSuggestion {
                word,
                probability: ((freq as f32 / denom) * 100.0).round() / 100.0,
                suggestion_type: "completion".to_string(),
            })
            .collect()
    }

    /// Predicts the next word using Trigram -> Bigram -> Unigram backoff.
    pub fn predict_next_word(&self, context: &[&str], top_k: usize) -> Vec<WordSuggestion> {
        if context.is_empty() {
            let mut top_unis: Vec<(&String, &u32)> = self.unigrams.iter().collect();
            top_unis.sort_by(|a, b| b.1.cmp(a.1));
            let total: u32 = top_unis.iter().take(top_k).map(|&(_, f)| f).sum();
            let denom = if total > 0 { total as f32 } else { 1.0 };

            return top_unis
                .into_iter()
                .take(top_k)
                .map(|(w, &f)| WordSuggestion {
                    word: w.clone(),
                    probability: ((f as f32 / denom) * 100.0).round() / 100.0,
                    suggestion_type: "next_word".to_string(),
                })
                .collect();
        }

        let words: Vec<String> = context.iter().map(|w| w.to_lowercase()).collect();

        // 1. Try Trigram
        if words.len() >= 2 {
            let w1 = &words[words.len() - 2];
            let w2 = &words[words.len() - 1];
            let key = (w1.clone(), w2.clone());
            if let Some(next_counts) = self.trigrams.get(&key) {
                let mut sorted: Vec<(&String, &u32)> = next_counts.iter().collect();
                sorted.sort_by(|a, b| b.1.cmp(a.1));
                let total: u32 = sorted.iter().map(|&(_, f)| f).sum();
                let denom = if total > 0 { total as f32 } else { 1.0 };

                return sorted
                    .into_iter()
                    .take(top_k)
                    .map(|(w, &f)| WordSuggestion {
                        word: w.clone(),
                        probability: ((f as f32 / denom) * 100.0).round() / 100.0,
                        suggestion_type: "next_word".to_string(),
                    })
                    .collect();
            }
        }

        // 2. Try Bigram
        if let Some(last_word) = words.last() {
            if let Some(next_counts) = self.bigrams.get(last_word) {
                let mut sorted: Vec<(&String, &u32)> = next_counts.iter().collect();
                sorted.sort_by(|a, b| b.1.cmp(a.1));
                let total: u32 = sorted.iter().map(|&(_, f)| f).sum();
                let denom = if total > 0 { total as f32 } else { 1.0 };

                return sorted
                    .into_iter()
                    .take(top_k)
                    .map(|(w, &f)| WordSuggestion {
                        word: w.clone(),
                        probability: ((f as f32 / denom) * 100.0).round() / 100.0,
                        suggestion_type: "next_word".to_string(),
                    })
                    .collect();
            }
        }

        // 3. Fallback to Unigram
        self.predict_next_word(&[], top_k)
    }

    fn train_default_corpus(&mut self) {
        let sentences = [
            "Xin chào bạn, chúc bạn một ngày làm việc thật vui vẻ và hiệu quả.",
            "Hôm nay bạn có khỏe không?",
            "Tôi rất vui được gặp bạn và trò chuyện cùng bạn.",
            "Cảm ơn bạn rất nhiều vì đã giúp đỡ tôi trong công việc.",
            "Xin lỗi vì đã làm phiền bạn vào lúc này.",
            "Bạn có rảnh không, chúng ta đi uống cà phê nhé.",
            "Tôi đang làm việc tại văn phòng công ty.",
            "Hôm nay trời rất đẹp, không khí mát mẻ và trong lành.",
            "Chúc bạn và gia đình luôn mạnh khỏe, bình an và hạnh phúc.",
            "Tạm biệt và hẹn gặp lại bạn vào ngày mai.",
            "Tôi muốn hỏi bạn một số thông tin về dự án mới.",
            "Bạn có thể gửi tài liệu qua email hoặc tin nhắn cho tôi được không?",
            "Chúng ta cần sắp xếp lại lịch họp vào chiều nay.",
            "Mọi việc đã được chuẩn bị chu đáo và sẵn sàng.",
            "Kết quả kiểm tra cho thấy hệ thống hoạt động rất ổn định.",
            "Tôi đồng ý với ý kiến đóng góp của bạn.",
            "Xin vui lòng chờ trong giây lát, tôi sẽ kiểm tra lại ngay.",
            "Bạn đã ăn cơm trưa chưa?",
            "Hệ thống tin nhắn hoạt động nhanh chóng và an toàn.",
            "Ứng dụng Mezon hỗ trợ gửi tin nhắn và gọi thoại chất lượng cao.",
            "Mô hình trí tuệ nhân tạo có thể hỗ trợ sửa lỗi chính tả tự động.",
            "Tính năng gợi ý từ tiếp theo giúp người dùng gõ phím nhanh hơn.",
            "Đội ngũ kỹ sư phần mềm đang tối ưu hóa hiệu năng ứng dụng.",
            "Chúng tôi luôn lắng nghe phản hồi của người dùng để cải thiện sản phẩm.",
            "Có công mài sắt, có ngày nên kim.",
            "Đi một ngày đàng, học một sàng khôn.",
            "Nói tiếng Việt không dấu anh em có hiểu không?",
            "Nói tiếng Việt có dấu giúp mọi người dễ đọc hơn.",
            "Anh em có hiểu ý của sếp không?",
            "Bạn có hiểu rõ nội dung thảo luận không?",
        ];
        self.train_on_corpus(&sentences);
    }
}
