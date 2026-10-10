using UnityEngine;

public static class TextUtil
{
    public static string Wrap(string t, int len = 40)
    {
        if (string.IsNullOrEmpty(t) || t.Length <= len)
        {
            return t;
        }

        int lastSpace = -1;
        System.Text.StringBuilder wrappedText = new System.Text.StringBuilder();

        for (int i = 0; i < t.Length; i++)
        {
            wrappedText.Append(t[i]);

            // Check if current position is beyond the line length
            if (i - lastSpace > len - 1)
            {
                // Insert a newline if there is a space after the lastSpace
                if (lastSpace != -1)
                {
                    wrappedText.Append("\n");
                }
                lastSpace = -1;
            }
            else if (char.IsWhiteSpace(t[i]))
            {
                lastSpace = i; // Update lastSpace
            }
        }

        return wrappedText.ToString();
    }
}