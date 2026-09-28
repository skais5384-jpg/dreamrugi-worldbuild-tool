#include <iostream>
#include <string>
#include <vector>
#include "hunspell.hxx"

// UTF-8 Korean tokens are supplied one per line. The caller owns tokenization,
// range mapping, input limits, and the process lifetime. No document is logged.
int main(int argc, char** argv) {
  if (argc != 3) return 2;
  Hunspell checker(argv[1], argv[2]);
  std::string word;
  while (std::getline(std::cin, word)) {
    if (word.empty() || word.size() > 1024 || word.find('\t') != std::string::npos)
      return 3;
    const bool valid = checker.spell(word) != 0;
    std::cout << word << '\t' << (valid ? '1' : '0');
    if (!valid) {
      const std::vector<std::string> suggestions = checker.suggest(word);
      size_t count = 0;
      for (const auto& suggestion : suggestions) {
        if (suggestion.find('\t') != std::string::npos ||
            suggestion.find('\n') != std::string::npos ||
            suggestion.find('\r') != std::string::npos) continue;
        std::cout << '\t' << suggestion;
        if (++count == 8) break;
      }
    }
    std::cout << '\n';
  }
  return std::cout.good() ? 0 : 4;
}
