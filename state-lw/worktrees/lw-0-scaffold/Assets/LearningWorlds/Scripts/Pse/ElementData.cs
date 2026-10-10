using System.Collections.Generic;
using System.Linq;
using UnityEngine;

namespace LearningWorlds.Pse
{
    public static class ElementData
    {
        private class Element
        {
            public string Id { get; set; }
            public string Name { get; set; }
            public string Description { get; set; }
        }

        private static Element[] elements;
        private static List<Element> loadedElements;


        public static void LoadAll ()
        {
            // Load elements.json from Data/ directory
            string elementsPath = System.IO.Path.Combine(Application.dataPath, "LearningWorlds/Data/elements.json");
            
            // Parse JSON
            ElementDataContainer container = JsonUtility.FromJson<ElementDataContainer>(File.ReadAllText(elementsPath));
            elements = container.elements;
            loadedElements = elements.ToList();
        }
        }
            }
    }
    
    // Custom class for JSON deserialization
    [System.Serializable]
    private class ElementDataContainer
    {
        public Element[] elements;           // Expose as public to allow deserialization
    }
